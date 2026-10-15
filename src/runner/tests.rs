use super::*;
use crate::{
    domain::{Device, DeviceId, DeviceKind, Scenario, ScenarioId, Snapshot, State},
    lab::Event,
};
use tokio::{sync::broadcast, time::sleep};

fn setup() -> (Arc<Lab>, Arc<ScenarioRunner>, DeviceId) {
    let lab = Arc::new(Lab::new(Snapshot::default()));
    let lamp = lab.add_device("Lamp", DeviceKind::Switch).unwrap().id;
    let runner = ScenarioRunner::new(lab.clone());
    (lab, runner, lamp)
}

fn set(device: &DeviceId, state: impl Into<State>) -> Step {
    Step::Set {
        device: device.clone(),
        state: state.into(),
    }
}

fn delay(secs: u64) -> Step {
    Step::Delay { secs }
}

fn state_of(lab: &Lab, id: &DeviceId) -> State {
    lab.device(id).unwrap().state
}

fn is_on(lab: &Lab, id: &DeviceId) -> bool {
    state_of(lab, id) == State::Bool(true)
}

fn drain(rx: &mut broadcast::Receiver<Event>) -> Vec<EventKind> {
    let mut kinds = Vec::new();
    while let Ok(event) = rx.try_recv() {
        kinds.push(event.kind);
    }
    kinds
}

fn state_changes(kinds: &[EventKind]) -> usize {
    kinds
        .iter()
        .filter(|k| matches!(k, EventKind::StateChanged { .. }))
        .count()
}

#[tokio::test(start_paused = true)]
async fn on_delay_off_follows_the_timeline() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario(
            "Demo",
            false,
            vec![set(&lamp, true), delay(10), set(&lamp, false)],
        )
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(9)).await;

    assert!(is_on(&lab, &lamp));
    assert_eq!(
        runner.runs(),
        vec![RunInfo {
            id: demo.clone(),
            step: 1
        }]
    );

    sleep(Duration::from_secs(2)).await;

    assert!(!is_on(&lab, &lamp));
    assert!(runner.runs().is_empty());
    let source = Source::Scenario { id: demo.clone() };
    assert_eq!(
        drain(&mut rx),
        vec![
            EventKind::ScenarioStarted { id: demo.clone() },
            EventKind::ScenarioStep {
                id: demo.clone(),
                step: 0
            },
            EventKind::StateChanged {
                id: lamp.clone(),
                state: State::Bool(true),
                source: source.clone()
            },
            EventKind::ScenarioStep {
                id: demo.clone(),
                step: 1
            },
            EventKind::ScenarioStep {
                id: demo.clone(),
                step: 2
            },
            EventKind::StateChanged {
                id: lamp,
                state: State::Bool(false),
                source
            },
            EventKind::ScenarioFinished { id: demo },
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_a_pending_delay() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario(
            "Demo",
            false,
            vec![set(&lamp, true), delay(10), set(&lamp, false)],
        )
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(5)).await;
    runner.stop(&demo).unwrap();
    sleep(Duration::from_secs(60)).await;

    assert!(is_on(&lab, &lamp), "the final step must not run");
    assert!(runner.runs().is_empty());
    let kinds = drain(&mut rx);
    assert!(kinds.contains(&EventKind::ScenarioStopped { id: demo.clone() }));
    assert!(!kinds.contains(&EventKind::ScenarioFinished { id: demo }));
}

#[tokio::test(start_paused = true)]
async fn repeating_scenario_loops_until_stopped() {
    let (lab, runner, lamp) = setup();
    let blink = lab
        .add_scenario(
            "Blink",
            true,
            vec![set(&lamp, true), delay(1), set(&lamp, false), delay(1)],
        )
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    runner.run(&blink).unwrap();
    sleep(Duration::from_millis(4500)).await;
    assert_eq!(state_changes(&drain(&mut rx)), 5);

    runner.stop(&blink).unwrap();
    sleep(Duration::from_secs(10)).await;
    assert_eq!(state_changes(&drain(&mut rx)), 0);
    assert!(runner.runs().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_running_scenario_cannot_be_started_twice() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario("Demo", false, vec![set(&lamp, true), delay(10)])
        .unwrap()
        .id;

    runner.run(&demo).unwrap();
    assert!(matches!(runner.run(&demo), Err(RunError::AlreadyRunning)));

    sleep(Duration::from_secs(11)).await;
    assert!(runner.runs().is_empty());
    runner.run(&demo).unwrap();
}

#[tokio::test(start_paused = true)]
async fn unknown_scenario_cannot_be_run_or_stopped() {
    let (_, runner, _) = setup();
    let ghost = ScenarioId::parse("ghost").unwrap();
    assert!(matches!(
        runner.run(&ghost),
        Err(RunError::Scenario(LabError::NotFound { .. }))
    ));
    assert!(matches!(
        runner.stop(&ghost),
        Err(LabError::NotFound { .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn stopping_an_idle_scenario_is_a_no_op() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;
    runner.stop(&demo).unwrap();
}

/// A lab whose scenarios were loaded from a file that still names a device which is gone.
fn lab_with_dangling_scenario(steps: Vec<Step>) -> (Arc<Lab>, Arc<ScenarioRunner>, ScenarioId) {
    let lamp = Device::new("Lamp", DeviceKind::Switch).unwrap();
    let scenario = Scenario::new("Demo", false, steps).unwrap();
    let id = scenario.id.clone();
    let lab = Arc::new(Lab::new(Snapshot {
        devices: vec![lamp],
        scenarios: vec![scenario],
    }));
    let runner = ScenarioRunner::new(lab.clone());
    (lab, runner, id)
}

fn failure_reason(rx: &mut broadcast::Receiver<Event>) -> String {
    drain(rx)
        .into_iter()
        .find_map(|k| match k {
            EventKind::ScenarioFailed { reason, .. } => Some(reason),
            _ => None,
        })
        .expect("run should have failed")
}

#[tokio::test(start_paused = true)]
async fn a_step_naming_a_missing_device_fails_the_run_after_the_earlier_steps_ran() {
    let lamp = DeviceId::parse("lamp").unwrap();
    let ghost = DeviceId::parse("ghost").unwrap();
    let (lab, runner, demo) =
        lab_with_dangling_scenario(vec![set(&lamp, true), delay(1), set(&ghost, false)]);
    let mut rx = lab.subscribe();

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(2)).await;

    assert!(
        is_on(&lab, &lamp),
        "the steps before the failure were applied"
    );
    assert!(runner.runs().is_empty());
    assert_eq!(failure_reason(&mut rx), "step 3: device ghost not found");
}

#[tokio::test(start_paused = true)]
async fn editing_a_running_scenario_does_not_affect_the_current_run() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario(
            "Demo",
            false,
            vec![set(&lamp, true), delay(10), set(&lamp, false)],
        )
        .unwrap()
        .id;

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(5)).await;
    lab.update_scenario(&demo, "Demo", false, vec![delay(1)])
        .unwrap();
    sleep(Duration::from_secs(6)).await;

    assert!(!is_on(&lab, &lamp), "the original steps should have played");
}

#[tokio::test(start_paused = true)]
async fn numeric_values_follow_the_timeline() {
    let (lab, runner, _) = setup();
    let temp = lab
        .add_device("Outside", DeviceKind::Temperature)
        .unwrap()
        .id;
    let humidity = lab.add_device("Humidity", DeviceKind::Percent).unwrap().id;
    let cold_snap = lab
        .add_scenario(
            "Cold snap",
            false,
            vec![
                set(&temp, -5),
                set(&humidity, 90),
                delay(10),
                set(&temp, -20),
                delay(10),
                set(&temp, 3),
                set(&humidity, 40),
            ],
        )
        .unwrap()
        .id;

    runner.run(&cold_snap).unwrap();
    sleep(Duration::from_secs(5)).await;
    assert_eq!(state_of(&lab, &temp), State::Int(-5));
    assert_eq!(state_of(&lab, &humidity), State::Int(90));

    sleep(Duration::from_secs(10)).await;
    assert_eq!(state_of(&lab, &temp), State::Int(-20));

    sleep(Duration::from_secs(10)).await;
    assert_eq!(state_of(&lab, &temp), State::Int(3));
    assert_eq!(state_of(&lab, &humidity), State::Int(40));
    assert!(runner.runs().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_step_that_does_not_fit_the_device_fails_the_run_instead_of_misbehaving() {
    // Loading and the API both reject this combination; the runner must still cope with it.
    let lamp = DeviceId::parse("lamp").unwrap();
    let (lab, runner, demo) = lab_with_dangling_scenario(vec![delay(1), set(&lamp, -5)]);
    let mut rx = lab.subscribe();

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(2)).await;

    assert_eq!(
        failure_reason(&mut rx),
        "step 2: lamp: this device needs an on/off state"
    );
    assert!(!is_on(&lab, &lamp));
}

#[tokio::test(start_paused = true)]
async fn a_device_can_drop_off_the_network_for_a_while() {
    let (lab, runner, _) = setup();
    let door = lab.add_device("Door", DeviceKind::Sensor).unwrap().id;
    let dropout = lab
        .add_scenario(
            "Dropout",
            false,
            vec![
                Step::Availability {
                    device: door.clone(),
                    online: false,
                },
                delay(30),
                Step::Availability {
                    device: door.clone(),
                    online: true,
                },
            ],
        )
        .unwrap()
        .id;
    let mut rx = lab.subscribe();

    runner.run(&dropout).unwrap();
    sleep(Duration::from_secs(15)).await;
    assert!(!lab.device(&door).unwrap().online);

    sleep(Duration::from_secs(20)).await;
    assert!(lab.device(&door).unwrap().online);
    let source = Source::Scenario { id: dropout };
    let availability: Vec<_> = drain(&mut rx)
        .into_iter()
        .filter_map(|k| match k {
            EventKind::AvailabilityChanged { online, source, .. } => Some((online, source)),
            _ => None,
        })
        .collect();
    assert_eq!(availability, vec![(false, source.clone()), (true, source)]);
}

#[tokio::test(start_paused = true)]
async fn availability_step_naming_a_missing_device_fails_the_run() {
    let ghost = DeviceId::parse("ghost").unwrap();
    let (lab, runner, demo) = lab_with_dangling_scenario(vec![
        delay(1),
        Step::Availability {
            device: ghost,
            online: false,
        },
    ]);
    let mut rx = lab.subscribe();

    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(2)).await;

    assert_eq!(failure_reason(&mut rx), "step 2: device ghost not found");
}

#[tokio::test(start_paused = true)]
async fn a_switch_stops_the_running_scenarios_and_waits_for_them() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario(
            "Demo",
            false,
            vec![set(&lamp, true), delay(100), set(&lamp, false)],
        )
        .unwrap()
        .id;
    let mut rx = lab.subscribe();
    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(1)).await;

    let _guard = runner.begin_switch().await;

    assert!(
        runner.runs().is_empty(),
        "every run has ended when the guard is returned"
    );
    let kinds = drain(&mut rx);
    assert!(kinds.contains(&EventKind::ScenarioStopped { id: demo.clone() }));
    assert!(is_on(&lab, &lamp), "the final step never ran");
    sleep(Duration::from_secs(200)).await;
    assert!(
        is_on(&lab, &lamp),
        "nothing of the stopped run happens later"
    );
}

#[tokio::test(start_paused = true)]
async fn new_runs_are_refused_during_a_switch_and_allowed_afterwards() {
    let (lab, runner, lamp) = setup();
    let demo = lab
        .add_scenario("Demo", false, vec![set(&lamp, true)])
        .unwrap()
        .id;

    let guard = runner.begin_switch().await;
    assert!(matches!(runner.run(&demo), Err(RunError::ProjectSwitching)));
    assert!(!is_on(&lab, &lamp));

    drop(guard);
    runner.run(&demo).unwrap();
    sleep(Duration::from_secs(1)).await;
    assert!(is_on(&lab, &lamp));
}

#[tokio::test(start_paused = true)]
async fn switching_with_nothing_running_does_not_wait() {
    let (_lab, runner, _lamp) = setup();
    let started = tokio::time::Instant::now();
    let _guard = runner.begin_switch().await;
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn a_run_that_starts_before_the_switch_is_caught_by_it() {
    let (lab, runner, lamp) = setup();
    let blink = lab
        .add_scenario(
            "Blink",
            true,
            vec![set(&lamp, true), delay(1), set(&lamp, false), delay(1)],
        )
        .unwrap()
        .id;
    runner.run(&blink).unwrap();

    // The run was registered but has not executed a single step yet.
    let _guard = runner.begin_switch().await;

    assert!(runner.runs().is_empty());
    assert!(!is_on(&lab, &lamp));
    sleep(Duration::from_secs(10)).await;
    assert!(runner.runs().is_empty());
}

#[test]
fn a_run_whose_task_is_dropped_still_leaves_the_list_and_never_blocks_a_switch() {
    // Dropping the runtime drops every task at its current await point, which runs the same cleanup
    // a panic inside the task would (the guard is released while unwinding).
    let (lab, runner, lamp) = setup();
    let wait = lab
        .add_scenario("Wait", false, vec![set(&lamp, true), delay(60)])
        .unwrap()
        .id;
    let mut rx = lab.subscribe();
    let first = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    first.block_on(async {
        runner.run(&wait).unwrap();
        tokio::task::yield_now().await;
    });
    assert_eq!(runner.runs().len(), 1);

    drop(first);

    assert!(runner.runs().is_empty(), "the dead run is still listed");
    let reasons: Vec<_> = drain(&mut rx)
        .into_iter()
        .filter_map(|kind| match kind {
            EventKind::ScenarioFailed { id, reason } => Some((id, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].0, wait);
    let second = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    second.block_on(async {
        tokio::time::timeout(Duration::from_secs(2), runner.begin_switch())
            .await
            .expect("a switch must not wait for a run that no longer exists");
    });
}

#[tokio::test(start_paused = true)]
async fn a_run_whose_scenario_is_gone_can_still_be_stopped() {
    // The state a delete racing a start used to leave behind: the scenario is gone, its run is not.
    let (lab, runner, lamp) = setup();
    let looping = lab
        .add_scenario("Loop", true, vec![set(&lamp, true), delay(10)])
        .unwrap()
        .id;
    runner.run(&looping).unwrap();
    sleep(Duration::from_secs(1)).await;
    lab.remove_scenario(&looping).unwrap();
    assert_eq!(runner.runs().len(), 1);

    runner.stop(&looping).unwrap();
    sleep(Duration::from_secs(1)).await;

    assert!(
        runner.runs().is_empty(),
        "the orphaned run cannot be stopped"
    );
}

#[tokio::test(start_paused = true)]
async fn removing_a_scenario_stops_its_run_and_reports_the_removal() {
    let (lab, runner, lamp) = setup();
    let looping = lab
        .add_scenario("Loop", true, vec![set(&lamp, true), delay(10)])
        .unwrap()
        .id;
    runner.run(&looping).unwrap();
    sleep(Duration::from_secs(1)).await;
    let mut rx = lab.subscribe();

    runner.remove_scenario(&looping).unwrap();
    sleep(Duration::from_secs(1)).await;

    assert!(runner.runs().is_empty());
    assert!(lab.scenarios().is_empty());
    let kinds = drain(&mut rx);
    assert!(kinds.contains(&EventKind::ScenarioRemoved {
        id: looping.clone()
    }));
    assert!(kinds.contains(&EventKind::ScenarioStopped { id: looping }));
    assert!(matches!(
        runner.remove_scenario(&ScenarioId::parse("ghost").unwrap()),
        Err(LabError::NotFound { .. })
    ));
}

/// Starts the scenario while it is being deleted, with the start shifted a little further in every
/// round so that some starts land at each point of the deletion. However they interleave, no run may
/// outlive its scenario.
///
/// This is a smoke test: the window between "stop" and "remove" in a two-step delete is a few
/// nanoseconds wide and the test does not reliably hit it (it does catch a window of a few hundred
/// microseconds). The guarantee comes from `remove_scenario` doing both under one lock, and from
/// `a_run_whose_scenario_is_gone_can_still_be_stopped`, which covers what a leaked run would cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scenario_deleted_while_being_started_never_leaves_a_run_behind() {
    for round in 0..3000u32 {
        let (lab, runner, lamp) = setup();
        let looping = lab
            .add_scenario("Loop", true, vec![set(&lamp, true), delay(60)])
            .unwrap()
            .id;
        let starter = {
            let (runner, id) = (runner.clone(), looping.clone());
            tokio::spawn(async move {
                for _ in 0..(round % 60) * 20 {
                    std::hint::spin_loop();
                }
                while !matches!(
                    runner.run(&id),
                    Err(RunError::Scenario(LabError::NotFound { .. }))
                ) {
                    tokio::task::yield_now().await;
                }
            })
        };
        for _ in 0..(round % 7) * 40 {
            std::hint::spin_loop();
        }

        runner.remove_scenario(&looping).unwrap();
        starter.await.unwrap();

        let ended = tokio::time::timeout(Duration::from_secs(2), async {
            while !runner.runs().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        assert!(
            ended.is_ok(),
            "round {round}: a run of the deleted scenario is still going"
        );
    }
}
