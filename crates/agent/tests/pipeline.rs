//! A pipeline end to end, against a stub model.
//!
//! What this proves: a pipeline is ordinary code; every model call goes
//! through the gate; a reply that does not fit its collar is retried and then
//! abandoned rather than guessed at; the budget is real; an outward effect
//! comes out as a proposal and nothing else; and the record holds identifiers
//! rather than a second copy of the content.

use std::sync::{Arc, Mutex};

use genatrix_agent::action::{Action, Approval, Effect};
use genatrix_agent::envelope::{Source, data_zone};
use genatrix_agent::output::{self, Summary};
use genatrix_agent::protocol::{Intent, RawReply, Structured, ToolProtocol};
use genatrix_agent::run::{
    CallError, Called, ModelCaller, ModelResult, RunContext, RunEnd, RunError, StepRecord,
};
use genatrix_gate::gate::{Initiator, Message, Request};
use genatrix_gate::redact::Identity;
use genatrix_keys::DbKey;
use genatrix_ledger::{EntryFilter, Ledger, kind};
use genatrix_llm::ticket::Purpose;
use genatrix_model::{ItemId, Level};

/// A model that answers from a script, and remembers what it was asked.
struct Stub {
    answers: Mutex<Vec<String>>,
    seen: Mutex<Vec<Request>>,
}

impl Stub {
    fn new(answers: &[&str]) -> Self {
        Self {
            answers: Mutex::new(answers.iter().rev().map(|s| (*s).to_owned()).collect()),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

impl ModelCaller for Stub {
    async fn call(&self, request: &Request) -> Result<Called, CallError> {
        self.seen.lock().unwrap().push(request.clone());
        let answer = self
            .answers
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| CallError::Unreachable("the stub ran out of answers".into()))?;
        Ok(Called {
            egress_id: format!("egress-{}", self.calls()),
            reply: RawReply::text(answer),
        })
    }
}

fn ledger() -> Arc<Ledger> {
    Arc::new(Ledger::open_in_memory(&DbKey::from_bytes([8; 32])).unwrap())
}

fn request(purpose: Purpose, items: &[ItemId], text: &str) -> Request {
    Request {
        purpose,
        initiator: Initiator::Rule {
            name: "daily-digest".into(),
        },
        items: items.to_vec(),
        level: Level::Personal,
        identities: vec![Identity {
            key: "p1".into(),
            names: vec!["Maria".into()],
        }],
        messages: vec![Message::system("Summarize."), Message::user(text)],
        max_tokens: Some(300),
        temperature: Some(0.2),
        stream: false,
    }
}

fn steps(ledger: &Ledger, run_id: &str) -> Vec<StepRecord> {
    ledger
        .entries(&EntryFilter {
            kind: Some(kind::RUN_STEP.into()),
            subject: Some(run_id.to_owned()),
            ..Default::default()
        })
        .unwrap()
        .iter()
        .map(|e| e.decode::<StepRecord>().unwrap())
        .collect()
}

#[tokio::test]
async fn a_pipeline_summarizes_and_proposes_a_reply() {
    let ledger = ledger();
    let item = ItemId::new();
    let id = item.to_string();
    let stub = Stub::new(&[
        &format!(
            r#"<think></think>{{"points":[{{"text":"Maria wants milestone two moved","sources":["{id}"]}}]}}"#
        ),
        r#"{"answer":"Maria, moving milestone two by two weeks is fine."}"#,
    ]);

    // The pipeline: ordinary code, fixed order, model fills blanks.
    let mut ctx = RunContext::begin(&ledger, &stub, "daily-digest", 8).unwrap();

    let zone = data_zone(&[Source {
        id: id.clone(),
        label: "mail from Maria".into(),
        body: "Please move milestone two by two weeks.".into(),
    }]);
    let allowed = zone.ids.clone();

    let summary: Summary = ctx
        .ask(
            "summarize",
            &request(Purpose::Summarize, &[item], &zone.text),
            |reply| output::summary(&reply.text, &allowed).map_err(|e| e.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(summary.points.len(), 1);
    assert!(summary.fully_grounded());

    let draft: String = ctx
        .ask(
            "draft",
            &request(Purpose::Draft, &[item], &zone.text),
            |reply| match Structured.interpret(reply) {
                Intent::Answer(text) => Ok(text),
                other => Err(format!("{other:?}")),
            },
        )
        .await
        .unwrap();

    ctx.propose(Action::propose(
        ctx.run_id.clone(),
        Effect::SendMail {
            account: "me@example.com".into(),
            to: vec!["maria@example.com".into()],
            subject: "Re: proposal".into(),
            in_reply_to: None,
            references: vec![],
        },
        draft,
        summary.points[0].text.clone(),
        vec![item],
    ))
    .unwrap();

    let run_id = ctx.run_id.clone();
    let actions = ctx.done().unwrap();

    assert_eq!(actions.len(), 1);
    assert!(
        matches!(actions[0].status, genatrix_agent::action::Status::Pending),
        "an outward effect leaves the pipeline as a proposal, nothing more"
    );
    assert_eq!(stub.calls(), 2);

    let recorded = steps(&ledger, &run_id);
    assert_eq!(recorded.len(), 2);
    for step in &recorded {
        match step {
            StepRecord::Model {
                egress,
                result,
                attempt,
                ..
            } => {
                assert_eq!(*result, ModelResult::Accepted);
                assert_eq!(*attempt, 1);
                assert!(egress.starts_with("egress-"));
            }
            other => panic!("{other:?}"),
        }
    }
    ledger.verify().unwrap();
}

#[tokio::test]
async fn the_run_record_holds_identifiers_not_a_second_copy_of_the_content() {
    let ledger = ledger();
    let item = ItemId::new();
    let secret_text = "Maria's private note about her salary";
    let stub = Stub::new(&[r#"{"answer":"ok"}"#]);
    let mut ctx = RunContext::begin(&ledger, &stub, "t", 4).unwrap();
    ctx.ask(
        "summarize",
        &request(Purpose::Summarize, &[item], secret_text),
        |r| Ok::<_, String>(r.text.clone()),
    )
    .await
    .unwrap();
    let run_id = ctx.run_id.clone();
    ctx.done().unwrap();

    let entries = ledger.entries(&EntryFilter::default()).unwrap();
    let bodies: String = entries.iter().map(|e| e.body.to_string()).collect();
    assert!(
        !bodies.contains("salary"),
        "the prompt text must not appear in the run record: {bodies}"
    );
    assert!(
        entries.iter().all(|e| e.subject == run_id),
        "every record points back at the run"
    );
    // What it does keep is the pointer to the egress record, which is where
    // the provenance and, if it left the device, the sent bytes live.
    assert!(bodies.contains("egress-1"));
}

#[tokio::test]
async fn a_reply_that_does_not_fit_is_retried_then_abandoned() {
    let ledger = ledger();
    let item = ItemId::new();
    let id = item.to_string();
    // First attempt is prose, second is a proper object.
    let stub = Stub::new(&[
        "I think the main point is that Maria wants a delay.",
        &format!(r#"{{"points":[{{"text":"a delay","sources":["{id}"]}}]}}"#),
    ]);
    let mut ctx = RunContext::begin(&ledger, &stub, "t", 8).unwrap();
    let allowed = vec![id.clone()];
    let summary = ctx
        .ask(
            "summarize",
            &request(Purpose::Summarize, &[item], "body"),
            |reply| output::summary(&reply.text, &allowed).map_err(|e| e.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(summary.points.len(), 1);
    let run_id = ctx.run_id.clone();
    assert_eq!(ctx.steps(), 2, "both attempts count against the budget");
    ctx.done().unwrap();

    let recorded = steps(&ledger, &run_id);
    assert_eq!(recorded.len(), 2);
    assert!(
        matches!(
            &recorded[0],
            StepRecord::Model {
                result: ModelResult::Malformed { .. },
                attempt: 1,
                ..
            }
        ),
        "the failed attempt is in the record, not swept away: {recorded:?}"
    );
}

#[tokio::test]
async fn a_reply_that_never_fits_stops_the_step_rather_than_being_guessed_at() {
    let ledger = ledger();
    let item = ItemId::new();
    let stub = Stub::new(&["prose", "still prose", "more prose"]);
    let mut ctx = RunContext::begin(&ledger, &stub, "t", 8).unwrap();
    let err = ctx
        .ask(
            "summarize",
            &request(Purpose::Summarize, &[item], "body"),
            |reply| output::summary(&reply.text, &[]).map_err(|e| e.to_string()),
        )
        .await
        .unwrap_err();
    match err {
        RunError::Unusable { attempts, step, .. } => {
            assert_eq!(attempts, 2);
            assert_eq!(step, "summarize");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(stub.calls(), 2, "it does not keep trying forever");
    ctx.stopped("nothing usable").unwrap();
}

#[tokio::test]
async fn the_step_budget_is_real() {
    let ledger = ledger();
    let item = ItemId::new();
    let stub = Stub::new(&[
        r#"{"answer":"a"}"#,
        r#"{"answer":"b"}"#,
        r#"{"answer":"c"}"#,
    ]);
    let mut ctx = RunContext::begin(&ledger, &stub, "t", 2).unwrap();
    let req = request(Purpose::Plan, &[item], "body");
    let collar = |r: &RawReply| Ok::<_, String>(r.text.clone());
    ctx.ask("one", &req, collar).await.unwrap();
    ctx.ask("two", &req, collar).await.unwrap();
    let err = ctx.ask("three", &req, collar).await.unwrap_err();
    assert!(matches!(err, RunError::OutOfSteps { max: 2 }), "{err:?}");
    assert_eq!(stub.calls(), 2, "the third call never went out");
}

#[tokio::test]
async fn a_run_that_proposes_nothing_still_leaves_a_trail() {
    let ledger = ledger();
    let stub = Stub::new(&[]);
    let ctx = RunContext::<Stub>::begin(&ledger, &stub, "empty", 4).unwrap();
    let run_id = ctx.run_id.clone();
    let actions = ctx.done().unwrap();
    assert!(actions.is_empty());

    let about = ledger
        .entries(&EntryFilter {
            subject: Some(run_id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(about.len(), 2);
    assert_eq!(about[0].kind, kind::RUN);
    assert_eq!(about[1].kind, kind::RUN_END);
    assert_eq!(
        about[1].decode::<RunEnd>().unwrap(),
        RunEnd::Done {
            steps: 0,
            actions: vec![]
        }
    );
}

#[tokio::test]
async fn a_proposed_action_still_needs_a_person() {
    let ledger = ledger();
    let stub = Stub::new(&[]);
    let mut ctx = RunContext::<Stub>::begin(&ledger, &stub, "t", 4).unwrap();
    let action = Action::propose(
        ctx.run_id.clone(),
        Effect::SendMessage {
            account: "42".into(),
            chat: "7".into(),
            reply_to: None,
        },
        "on my way",
        "she asked where you are",
        vec![ItemId::new()],
    );
    let id = action.id.clone();
    ctx.propose(action).unwrap();
    let mut actions = ctx.done().unwrap();

    // Nothing the pipeline produced can be executed as it stands.
    let action = &mut actions[0];
    assert!(
        action
            .begin_execution(
                &genatrix_agent::action::ExecutionToken::default_invalid(),
                chrono::Utc::now()
            )
            .is_err()
    );

    // It takes an approval carrying the version the user looked at.
    let approval = Approval {
        version: action.current().seq,
        payload_hash: action.current().payload_hash.clone(),
    };
    let token = action.approve(&approval, chrono::Utc::now()).unwrap();
    let version = action.begin_execution(&token, chrono::Utc::now()).unwrap();
    assert_eq!(version.payload, "on my way");

    // And it is all in the ledger.
    let entries = ledger
        .entries(&EntryFilter {
            kind: Some(kind::ACTION.into()),
            subject: Some(id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(entries.len(), 1);
}
