use actix_web::body::MessageBody;
use actix_web::http::StatusCode;
use actix_web::{App, test as http, web};
use intelligence_engine::action::{Action, ProposedAction, Source, Suggestion};
use intelligence_engine::api;
use intelligence_engine::db;
use intelligence_engine::engine::{Config, Engine, Step, step};
use intelligence_engine::model::{DecisionModel, IdentityModel};
use intelligence_engine::vocabulary::Vocabulary;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn text_editor_schema() -> Value {
    serde_json::from_str(include_str!("../examples/text-editor.schema.json")).unwrap()
}

fn temp_db() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "intelligence-engine-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("engine.sqlite")
}

fn config(tick_ms: u64) -> Config {
    Config {
        db_path: temp_db(),
        tick: Duration::from_millis(tick_ms),
        ..Config::default()
    }
}

fn action(user: &str, name: &str, payload: Value) -> Action {
    Action {
        user_id: user.into(),
        session_id: "s1".into(),
        action_name: name.into(),
        payload,
        source: Source::User,
    }
}

fn write(user: &str, n: usize) -> Action {
    action(
        user,
        "write_text",
        json!({ "id": "note-1", "text": format!("draft {n}") }),
    )
}

macro_rules! app {
    ($engine:expr) => {
        http::init_service(
            App::new()
                .app_data(web::Data::from(Arc::clone(&$engine)))
                .configure(api::configure),
        )
        .await
    };
}

#[actix_rt::test]
async fn init_and_action_validation() {
    let engine = Engine::open(config(60_000), Arc::new(IdentityModel)).unwrap();
    let app = app!(engine);

    let post = |uri: &str, body: Value| {
        http::TestRequest::post()
            .uri(uri)
            .set_json(body)
            .to_request()
    };

    let res = http::call_service(&app, post("/action", json!(write("u", 1)))).await;
    assert_eq!(
        res.status(),
        StatusCode::CONFLICT,
        "actions need a vocabulary first"
    );

    let res = http::call_service(&app, post("/init", json!({ "type": 12 }))).await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = http::call_service(&app, post("/init", text_editor_schema())).await;
    assert_eq!(res.status(), StatusCode::OK);

    let res = http::call_service(&app, post("/action", json!(write("u", 1)))).await;
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let body: Value = http::read_body_json(res).await;
    assert!(body["id"].as_i64().unwrap() > 0);

    let wrong = action("u", "write_text", json!({ "id": "note-1" }));
    let res = http::call_service(&app, post("/action", json!(wrong))).await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = http::read_body_json(res).await;
    assert!(!body["details"].as_array().unwrap().is_empty(), "{body}");

    let res = http::call_service(
        &app,
        post("/action", json!({ "user_id": "u", "action_name": "save" })),
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "missing session_id and source"
    );

    let res = http::call_service(&app, post("/action", json!({ "user_id": " ", "session_id": "s", "action_name": "create", "payload": { "id": "a" }, "source": "user" }))).await;
    assert_eq!(
        res.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "blank user_id"
    );

    let res = http::call_service(
        &app,
        http::TestRequest::get().uri("/sse/suggestion").to_request(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST, "user_id is required");
    engine.shutdown();
}

#[test]
fn step_runs_the_model_every_five_new_actions() {
    let cfg = Config {
        max_window: 3,
        ..config(0)
    };
    let conn = db::open(&cfg.db_path).unwrap();
    let vocabulary = Vocabulary::new(text_editor_schema()).unwrap();
    let run = |last| step(&conn, "u", last, &vocabulary, &IdentityModel, &cfg).unwrap();

    for n in 0..4 {
        db::insert_action(&conn, &write("u", n)).unwrap();
    }
    db::insert_action(&conn, &write("someone-else", 0)).unwrap();
    assert_eq!(
        run(None),
        Step::Waiting,
        "4 actions is not enough, other users don't count"
    );

    let last = db::insert_action(&conn, &write("u", 4)).unwrap();
    let Step::Processed {
        last: processed,
        suggestion: Some(suggestion),
    } = run(None)
    else {
        panic!("expected a suggestion")
    };
    assert_eq!(processed, last);
    assert_eq!(suggestion.user_id, "u");
    assert_eq!(suggestion.confidence, 0.0);
    // Identity over the newest `max_window` actions, re-labelled as system actions.
    let texts: Vec<_> = suggestion
        .actions
        .iter()
        .map(|a| a.payload["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, ["draft 2", "draft 3", "draft 4"]);
    assert!(
        suggestion
            .actions
            .iter()
            .all(|a| a.source == Source::System && a.action_name == "write_text")
    );

    for n in 5..9 {
        db::insert_action(&conn, &write("u", n)).unwrap();
    }
    assert_eq!(
        run(Some(processed)),
        Step::Waiting,
        "only 4 new since the last run"
    );
}

struct Proposes(Vec<ProposedAction>, f32);

impl DecisionModel for Proposes {
    fn predict(&self, _: &Value, _: &[Action]) -> (f32, Vec<ProposedAction>) {
        (self.1, self.0.clone())
    }
}

#[test]
fn step_drops_proposals_outside_the_vocabulary_and_clamps_confidence() {
    let cfg = config(0);
    let conn = db::open(&cfg.db_path).unwrap();
    let vocabulary = Vocabulary::new(text_editor_schema()).unwrap();
    for n in 0..5 {
        db::insert_action(&conn, &write("u", n)).unwrap();
    }

    let model = Proposes(
        vec![
            ProposedAction::new("explode", json!({})),
            ProposedAction::new("save", json!({ "id": "a", "text": "x" })),
        ],
        7.0,
    );
    let Step::Processed {
        suggestion: Some(s),
        ..
    } = step(&conn, "u", None, &vocabulary, &model, &cfg).unwrap()
    else {
        panic!()
    };
    assert_eq!(s.actions.len(), 1);
    assert_eq!(s.actions[0].action_name, "save");
    assert_eq!(s.confidence, 1.0);

    let model = Proposes(vec![ProposedAction::new("explode", json!({}))], f32::NAN);
    let result = step(&conn, "u", None, &vocabulary, &model, &cfg).unwrap();
    assert!(
        matches!(
            result,
            Step::Processed {
                suggestion: None,
                ..
            }
        ),
        "nothing valid, but the actions still count as processed"
    );
}

#[test]
fn vocabulary_survives_a_restart_and_reopening_keeps_data() {
    let cfg = config(60_000);
    let engine = Engine::open(cfg.clone(), Arc::new(IdentityModel)).unwrap();
    engine.init(text_editor_schema()).unwrap();
    engine.record(&write("u", 0)).unwrap();
    engine.shutdown();
    drop(engine);

    let reopened = Engine::open(cfg.clone(), Arc::new(IdentityModel)).unwrap();
    assert_eq!(reopened.vocabulary().unwrap().schema, text_editor_schema());
    let conn = db::open(&cfg.db_path).unwrap();
    assert_eq!(db::count_after(&conn, "u", 0).unwrap(), 1);
    reopened.shutdown();
}

/// Next `suggestion` event on an SSE body, skipping comments.
async fn next_suggestion<B: MessageBody + Unpin>(body: &mut B) -> Suggestion {
    let mut buffer = String::new();
    loop {
        let chunk =
            futures_util::future::poll_fn(|cx| std::pin::Pin::new(&mut *body).poll_next(cx))
                .await
                .expect("stream ended")
                .unwrap_or_else(|_| panic!("stream error"));
        buffer.push_str(std::str::from_utf8(&chunk).unwrap());
        if let Some(data) = buffer
            .split("\n\n")
            .find_map(|frame| frame.strip_prefix("event: suggestion\ndata: "))
        {
            return serde_json::from_str(data).unwrap();
        }
    }
}

#[actix_rt::test]
async fn actions_in_suggestion_out_over_sse() {
    let engine = Engine::open(config(50), Arc::new(IdentityModel)).unwrap();
    let app = app!(engine);
    engine.init(text_editor_schema()).unwrap();

    let res = http::call_service(
        &app,
        http::TestRequest::get()
            .uri("/sse/suggestion?user_id=sam")
            .to_request(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    let mut body = res.into_body();

    for n in 0..5 {
        let req = http::TestRequest::post()
            .uri("/action")
            .set_json(json!(write("sam", n)))
            .to_request();
        assert_eq!(
            http::call_service(&app, req).await.status(),
            StatusCode::ACCEPTED
        );
    }

    let suggestion = tokio::time::timeout(Duration::from_secs(5), next_suggestion(&mut body))
        .await
        .expect("no suggestion within 5s");
    assert_eq!(suggestion.user_id, "sam");
    assert_eq!(suggestion.actions.len(), 5);
    assert_eq!(
        suggestion.actions[4].payload,
        json!({ "id": "note-1", "text": "draft 4" })
    );
    engine.shutdown();
}

#[actix_rt::test]
async fn a_stale_stream_does_not_swallow_suggestions() {
    let engine = Engine::open(config(50), Arc::new(IdentityModel)).unwrap();
    let app = app!(engine);
    engine.init(text_editor_schema()).unwrap();
    let open = || {
        http::TestRequest::get()
            .uri("/sse/suggestion?user_id=sam")
            .to_request()
    };

    // A tab that was replaced: the server keeps polling its stream (writing into a dead socket)
    // until a write fails, so it actively takes whatever it's given.
    let mut stale = http::call_service(&app, open()).await.into_body();
    actix_rt::spawn(async move {
        while let Some(Ok(_)) =
            futures_util::future::poll_fn(|cx| std::pin::Pin::new(&mut stale).poll_next(cx)).await
        {
        }
    });
    let mut fresh = http::call_service(&app, open()).await.into_body();

    for n in 0..5 {
        let req = http::TestRequest::post()
            .uri("/action")
            .set_json(json!(write("sam", n)))
            .to_request();
        http::call_service(&app, req).await;
    }
    let suggestion = tokio::time::timeout(Duration::from_secs(5), next_suggestion(&mut fresh))
        .await
        .expect("the live stream got nothing");
    assert_eq!(suggestion.actions.len(), 5);
    engine.shutdown();
}
