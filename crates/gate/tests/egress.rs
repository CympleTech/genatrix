//! The gate's promises from design 02, and the handshake between the two
//! layers: a ticket the gate mints is accepted by the gateway that checks it,
//! for exactly those bytes and nothing else.

use std::collections::BTreeMap;
use std::sync::Arc;

use genatrix_gate::gate::{
    Decision, EgressGate, EgressRecord, GateError, Initiator, Message, Outcome, Request,
};
use genatrix_gate::redact::Identity;
use genatrix_gate::rules::RuleSet;
use genatrix_keys::{DbKey, TicketKey};
use genatrix_ledger::{EntryFilter, Ledger, kind};
use genatrix_llm::registry::{ALL_PURPOSES, Endpoint, Location, ModelEntry, Registry};
use genatrix_llm::ticket::{Purpose, TicketLevel, TicketStore};
use genatrix_model::{ItemId, Level};

fn ticket_key() -> TicketKey {
    TicketKey::from_bytes([17; 32])
}

fn local(purposes: Vec<Purpose>) -> ModelEntry {
    ModelEntry {
        name: "local-qwen3".into(),
        endpoint: Endpoint::LocalSocket {
            path: "/tmp/gx/infer.sock".into(),
        },
        model: "qwen3-8b-4bit".into(),
        purposes,
        context_length: 32_768,
    }
}

fn cloud(purposes: Vec<Purpose>) -> ModelEntry {
    ModelEntry {
        name: "cloud-sonnet".into(),
        endpoint: Endpoint::Anthropic {
            base_url: "https://api.anthropic.com".into(),
            key_ref: "anthropic".into(),
        },
        model: "claude-sonnet-5".into(),
        purposes,
        context_length: 200_000,
    }
}

fn registry() -> Registry {
    let r = Registry {
        models: vec![
            local(ALL_PURPOSES.to_vec()),
            cloud(vec![Purpose::Summarize, Purpose::Draft, Purpose::Translate]),
        ],
        chains: BTreeMap::new(),
    };
    r.validate().unwrap();
    r
}

struct Fixture {
    gate: EgressGate,
    ledger: Arc<Ledger>,
}

fn fixture(cloud_enabled: bool) -> Fixture {
    let ledger = Arc::new(Ledger::open_in_memory(&DbKey::from_bytes([4; 32])).unwrap());
    let gate = EgressGate::new(RuleSet::builtin(), registry(), ledger.clone(), ticket_key())
        .with_cloud(cloud_enabled);
    Fixture { gate, ledger }
}

fn request(purpose: Purpose, level: Level, text: &str) -> Request {
    Request {
        purpose,
        initiator: Initiator::Agent {
            run: "run-1".into(),
        },
        items: vec![ItemId::new()],
        level,
        identities: vec![Identity {
            key: "p1".into(),
            names: vec!["Alice Chen".into(), "alice@example.com".into()],
        }],
        messages: vec![
            Message::system("Summarize the message."),
            Message::user(text),
        ],
        max_tokens: Some(200),
        temperature: Some(0.2),
        stream: false,
    }
}

fn egress_records(ledger: &Ledger) -> Vec<EgressRecord> {
    ledger
        .entries(&EntryFilter {
            kind: Some(kind::EGRESS.into()),
            ..Default::default()
        })
        .unwrap()
        .iter()
        .map(|e| e.decode::<EgressRecord>().unwrap())
        .collect()
}

#[test]
fn a_local_call_is_recorded_without_a_second_copy_of_the_content() {
    let f = fixture(false);
    let p = f
        .gate
        .prepare(&request(
            Purpose::Summarize,
            Level::Personal,
            "Alice Chen asked about Friday.",
        ))
        .unwrap();

    assert_eq!(p.location, Location::Local);
    assert_eq!(p.decision, Decision::Allowed);
    assert!(p.redaction.is_none(), "nothing leaves, nothing is reduced");
    assert!(
        String::from_utf8_lossy(&p.body).contains("Alice Chen"),
        "a local model sees the real names"
    );

    let records = egress_records(&f.ledger);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].location, "local");
    assert_eq!(records[0].level, "personal");
    assert!(
        records[0].payload.is_none(),
        "a local call records provenance, not a duplicate of the database"
    );
    assert_eq!(records[0].payload_hash.len(), 64);
}

#[test]
fn content_that_leaves_is_redacted_and_recorded_in_full() {
    let f = fixture(true);
    let p = f
        .gate
        .prepare(&request(
            Purpose::Summarize,
            Level::Personal,
            "Alice Chen asked about Friday; reply to alice@example.com.",
        ))
        .unwrap();

    assert_eq!(p.location, Location::Cloud);
    assert_eq!(p.decision, Decision::AllowedRedacted);
    let sent = String::from_utf8_lossy(&p.body).into_owned();
    assert!(!sent.contains("Alice Chen"), "{sent}");
    assert!(!sent.contains("alice@example.com"), "{sent}");
    assert!(sent.contains("[Person 1]"), "{sent}");

    let records = egress_records(&f.ledger);
    let payload = records[0]
        .payload
        .as_ref()
        .expect("cloud calls record the bytes");
    assert_eq!(
        payload, &sent,
        "the record holds exactly what the gateway will be handed"
    );
    assert!(!payload.contains("Alice Chen"));

    // And the reply comes back readable on this machine.
    let redaction = p.redaction.expect("a redacted call keeps its map");
    let restored = redaction.restore("I told [Person 1] that Friday works.", |k| {
        (k == "p1").then(|| "Alice Chen".to_owned())
    });
    assert_eq!(restored, "I told Alice Chen that Friday works.");
}

#[test]
fn secret_content_falls_back_to_the_local_model() {
    let f = fixture(true);
    let p = f
        .gate
        .prepare(&request(
            Purpose::Draft,
            Level::Secret,
            "HbA1c 7.1%, follow up with the clinic.",
        ))
        .unwrap();
    assert_eq!(p.location, Location::Local);
    assert_eq!(p.target, "local-qwen3");
    let records = egress_records(&f.ledger);
    assert_eq!(records[0].decision, Decision::FallbackLocal);
    assert!(records[0].payload.is_none());
}

#[test]
fn switching_the_cloud_off_sends_everything_local() {
    let f = fixture(false);
    let p = f
        .gate
        .prepare(&request(Purpose::Draft, Level::Public, "a public notice"))
        .unwrap();
    assert_eq!(p.location, Location::Local);
    assert_eq!(
        egress_records(&f.ledger)[0].decision,
        Decision::Allowed,
        "with the cloud off, local is the configuration, not a fallback"
    );
}

#[test]
fn an_installed_agent_stays_local_unless_its_own_switch_is_on() {
    // Design 11, ruling 12: the global switch is not enough for an agent.
    let f = fixture(true);
    let mut r = request(Purpose::Summarize, Level::Public, "a public notice");
    r.initiator = Initiator::Installed {
        agent: "a1".into(),
        version: "h1".into(),
        run: "r1".into(),
        cloud: false,
    };
    assert_eq!(f.gate.prepare(&r).unwrap().location, Location::Local);
    r.initiator = Initiator::Installed {
        agent: "a1".into(),
        version: "h1".into(),
        run: "r2".into(),
        cloud: true,
    };
    assert_eq!(f.gate.prepare(&r).unwrap().location, Location::Cloud);
}

#[test]
fn a_corpus_purpose_never_resolves_to_the_cloud() {
    let f = fixture(true);
    for purpose in [Purpose::Classify, Purpose::Extract, Purpose::Embed] {
        let p = f
            .gate
            .prepare(&request(purpose, Level::Public, "anything"))
            .unwrap();
        assert_eq!(p.location, Location::Local, "{purpose:?}");
    }
}

#[test]
fn the_gateway_accepts_the_gate_s_ticket_for_these_bytes_and_no_others() {
    let f = fixture(true);
    let p = f
        .gate
        .prepare(&request(
            Purpose::Summarize,
            Level::Personal,
            "Alice Chen asked about Friday.",
        ))
        .unwrap();

    // The other layer, with only the shared key and the registry.
    let store = TicketStore::new();
    let admitted = store
        .admit(Some(&p.ticket), &p.body, &p.target, &ticket_key())
        .expect("the gateway accepts what the gate minted");
    assert_eq!(admitted.purpose, Purpose::Summarize);
    assert_eq!(admitted.level, TicketLevel::Redacted);

    // Same ticket, different bytes.
    let tampered = bytes::Bytes::from_static(b"{\"model\":\"cloud-sonnet\",\"messages\":[]}");
    assert!(
        store
            .admit(Some(&p.ticket), &tampered, &p.target, &ticket_key())
            .is_err()
    );
}

#[test]
fn a_ticket_for_a_local_call_says_so() {
    let f = fixture(true);
    let p = f
        .gate
        .prepare(&request(Purpose::Draft, Level::Secret, "secret matters"))
        .unwrap();
    let admitted = TicketStore::new()
        .admit(Some(&p.ticket), &p.body, &p.target, &ticket_key())
        .unwrap();
    assert_eq!(
        admitted.level,
        TicketLevel::Secret,
        "the ticket tells the truth about the content, not about the destination"
    );
}

#[test]
fn the_outcome_is_appended_after_the_record() {
    let f = fixture(false);
    let p = f
        .gate
        .prepare(&request(Purpose::Summarize, Level::Personal, "hello"))
        .unwrap();
    f.gate
        .record_outcome(
            &p.egress_id,
            &Outcome::Sent {
                response_hash: EgressGate::hash_response(b"the reply"),
            },
        )
        .unwrap();

    let about = f
        .ledger
        .entries(&EntryFilter {
            subject: Some(p.egress_id.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(about.len(), 2);
    assert_eq!(about[0].kind, kind::EGRESS);
    assert_eq!(about[1].kind, kind::EGRESS_RESULT);
    assert!(
        about[0].seq < about[1].seq,
        "the record precedes the result"
    );
    f.ledger.verify().unwrap();
}

#[test]
fn a_purpose_with_no_model_is_refused_rather_than_sent_somewhere_else() {
    let ledger = Arc::new(Ledger::open_in_memory(&DbKey::from_bytes([4; 32])).unwrap());
    // A registry that serves only drafting.
    let registry = Registry {
        models: vec![local(vec![Purpose::Draft])],
        chains: BTreeMap::new(),
    };
    let gate = EgressGate::new(RuleSet::builtin(), registry, ledger.clone(), ticket_key());
    let err = gate
        .prepare(&request(Purpose::Summarize, Level::Personal, "hello"))
        .unwrap_err();
    assert!(matches!(err, GateError::NoModel { .. }), "{err:?}");
    assert!(
        ledger.is_empty().unwrap(),
        "a refusal before any decision writes nothing"
    );
}

#[test]
fn a_pasted_verification_code_makes_the_users_own_words_secret() {
    let f = fixture(true);
    assert_eq!(
        f.gate
            .level_of_typed_text("here is the code: 482913, what does it mean?"),
        Level::Secret
    );
    assert_eq!(
        f.gate
            .level_of_typed_text("what did Alice say about Friday?"),
        Level::Personal
    );
}
