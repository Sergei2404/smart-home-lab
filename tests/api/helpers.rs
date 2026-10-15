use reqwest::Response;
use serde_json::{Value, json};
use smart_home_lab::{
    app::App,
    domain::{ProjectId, Snapshot},
    lab::Lab,
    projects::{persister::spawn_persister, store::ProjectStore},
    web,
};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub struct TestApp {
    pub address: String,
    pub client: reqwest::Client,
    pub lab: Arc<Lab>,
    pub store: Arc<ProjectStore>,
    /// Cancel it to shut the server down, as the binary does on Ctrl-C.
    pub shutdown: CancellationToken,
    pub server: JoinHandle<std::io::Result<()>>,
    _dir: tempfile::TempDir,
}

/// An app whose changes stay in memory: nothing writes the project files in the background, so
/// a test can tell what is live from what is on disk.
pub async fn spawn_app() -> TestApp {
    start(false).await
}

/// Like `spawn_app`, but the persister runs as it does in the real binary.
pub async fn spawn_app_with_persister() -> TestApp {
    start(true).await
}

async fn start(persist: bool) -> TestApp {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind random port");
    let address = format!("http://{}", listener.local_addr().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        ProjectStore::open(dir.path().join("projects"))
            .await
            .unwrap(),
    );
    store.ensure_default().await.unwrap();
    let lab = Arc::new(Lab::new(Snapshot::default()));
    if persist {
        // Never stopped: it ends with the test's runtime.
        spawn_persister(store.clone(), lab.snapshots(), CancellationToken::new());
    }
    let app = App::new(lab.clone(), store.clone());
    let shutdown = app.shutdown.clone();
    let server = tokio::spawn(web::serve(listener, app));

    TestApp {
        address,
        client: reqwest::Client::new(),
        lab,
        store,
        shutdown,
        server,
        _dir: dir,
    }
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.address)
    }

    pub async fn get(&self, path: &str) -> Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }

    pub async fn create_device(&self, name: &str, kind: &str) -> Response {
        self.client
            .post(self.url("/api/devices"))
            .json(&json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap()
    }

    pub async fn set_state(&self, id: &str, state: impl serde::Serialize) -> Response {
        self.client
            .put(self.url(&format!("/api/devices/{id}/state")))
            .json(&json!({ "state": state }))
            .send()
            .await
            .unwrap()
    }

    pub async fn set_availability(&self, id: &str, online: bool) -> Response {
        self.client
            .put(self.url(&format!("/api/devices/{id}/availability")))
            .json(&json!({ "online": online }))
            .send()
            .await
            .unwrap()
    }

    pub async fn delete_device(&self, id: &str) -> Response {
        self.client
            .delete(self.url(&format!("/api/devices/{id}")))
            .send()
            .await
            .unwrap()
    }

    pub async fn devices(&self) -> Vec<Value> {
        self.get("/api/devices").await.json().await.unwrap()
    }

    pub async fn create_scenario(&self, body: Value) -> Response {
        self.client
            .post(self.url("/api/scenarios"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn update_scenario(&self, id: &str, body: Value) -> Response {
        self.client
            .put(self.url(&format!("/api/scenarios/{id}")))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn delete_scenario(&self, id: &str) -> Response {
        self.client
            .delete(self.url(&format!("/api/scenarios/{id}")))
            .send()
            .await
            .unwrap()
    }

    /// Waits until the file of `project` satisfies `ready` and returns its content (the persister
    /// writes in the background, so a file is only eventually up to date).
    pub async fn saved(&self, project: &str, ready: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let id = ProjectId::parse(project).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(snapshot) = self.store.load(&id).await
                    && ready(&snapshot)
                {
                    break snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("the file of project {project} never reached the expected content")
        })
    }

    pub async fn projects(&self) -> Value {
        self.get("/api/projects").await.json().await.unwrap()
    }

    pub async fn create_project(&self, body: Value) -> Response {
        self.client
            .post(self.url("/api/projects"))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    pub async fn activate_project(&self, id: &str) -> Response {
        self.post(&format!("/api/projects/{id}/activate")).await
    }

    pub async fn delete_project(&self, id: &str) -> Response {
        self.client
            .delete(self.url(&format!("/api/projects/{id}")))
            .send()
            .await
            .unwrap()
    }

    pub async fn post(&self, path: &str) -> Response {
        self.client.post(self.url(path)).send().await.unwrap()
    }

    pub async fn runs(&self) -> Vec<Value> {
        self.get("/api/runs").await.json().await.unwrap()
    }
}

pub async fn read_until(response: &mut Response, needle: &str) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut received = String::new();
        while let Some(chunk) = response.chunk().await.unwrap() {
            received.push_str(&String::from_utf8_lossy(&chunk));
            if received.contains(needle) {
                break;
            }
        }
        received
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}"))
}
