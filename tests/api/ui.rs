use crate::helpers::spawn_app;

#[tokio::test]
async fn index_page_is_served() {
    let app = spawn_app().await;
    let response = app.get("/").await;

    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert!(response.text().await.unwrap().contains("Smart Home Lab"));
}

#[tokio::test]
async fn static_assets_have_correct_content_types() {
    let app = spawn_app().await;
    for (path, content_type) in [("/app.js", "text/javascript"), ("/style.css", "text/css")] {
        let response = app.get(path).await;
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with(content_type),
            "{path}"
        );
    }
}
