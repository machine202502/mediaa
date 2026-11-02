use mediaa_sync::{
    Client, ClientUpdate, EventLog, Frame, Server, ServerAction, StateHub, StateView,
    PROTOCOL_VERSION,
};
use std::collections::BTreeMap;

fn frames(action: ServerAction) -> Vec<Frame> {
    match action {
        ServerAction::Frames(frames) => frames,
        ServerAction::Command(request) => panic!("unexpected command: {:?}", request.name),
    }
}

fn hello(server: &mut Server) -> Vec<Frame> {
    frames(server.on_frame(Frame::Hello {
        version: PROTOCOL_VERSION,
        client: "test".into(),
    }))
}

fn subscribe(
    server: &mut Server,
    stores: &[&str],
    versions: &[(&str, u64)],
    event_seq: u64,
) -> Vec<Frame> {
    frames(
        server.on_frame(Frame::Subscribe {
            stores: stores.iter().map(|store| store.to_string()).collect(),
            versions: versions
                .iter()
                .map(|(store, version)| (store.to_string(), *version))
                .collect::<BTreeMap<_, _>>(),
            event_seq,
        }),
    )
}

#[test]
fn subscribe_sends_a_snapshot_of_known_state() {
    let mut server = Server::new();
    server.hub_mut().publish("settings", 3, b"state".to_vec());
    hello(&mut server);
    assert_eq!(
        subscribe(&mut server, &["settings"], &[], 0),
        vec![Frame::Snapshot {
            store: "settings".into(),
            version: 3,
            payload: b"state".to_vec(),
        }]
    );
}

#[test]
fn a_current_client_gets_no_snapshot_and_later_changes() {
    let mut server = Server::new();
    server.hub_mut().publish("settings", 3, b"state".to_vec());
    hello(&mut server);
    assert!(subscribe(&mut server, &["settings"], &[("settings", 3)], 0).is_empty());
    server.hub_mut().publish("settings", 4, b"new".to_vec());
    assert_eq!(
        server.flush(),
        vec![Frame::Changed {
            store: "settings".into(),
            version: 4,
            payload: b"new".to_vec(),
        }]
    );
    assert!(server.flush().is_empty());
}

#[test]
fn unsubscribed_and_coalesced_changes_are_not_sent() {
    let mut server = Server::new();
    hello(&mut server);
    subscribe(&mut server, &["recording"], &[], 0);
    server.hub_mut().publish("other", 1, b"x".to_vec());
    assert!(server.flush().is_empty());
    server.hub_mut().publish("recording", 1, b"saving".to_vec());
    server.hub_mut().publish("recording", 2, b"idle".to_vec());
    assert_eq!(
        server.flush(),
        vec![Frame::Changed {
            store: "recording".into(),
            version: 2,
            payload: b"idle".to_vec(),
        }]
    );
}

#[test]
fn resync_returns_fresh_snapshots() {
    let mut server = Server::new();
    server.hub_mut().publish("library", 9, b"entries".to_vec());
    hello(&mut server);
    subscribe(&mut server, &["library"], &[("library", 9)], 0);
    assert_eq!(
        frames(server.on_frame(Frame::Resync {
            stores: vec!["library".into()],
            event_seq: 0,
        })),
        vec![Frame::Snapshot {
            store: "library".into(),
            version: 9,
            payload: b"entries".to_vec(),
        }]
    );
}

#[test]
fn commands_are_acknowledged_and_deduplicated() {
    let mut server = Server::new();
    hello(&mut server);
    let ServerAction::Command(request) = server.on_frame(Frame::Command {
        id: 7,
        name: "echo".into(),
        payload: b"x".to_vec(),
    }) else {
        panic!("expected command");
    };
    let result = server.complete_command(request.id, Ok(b"x".to_vec()));
    assert_eq!(
        result,
        Frame::Result {
            id: 7,
            ok: true,
            payload: b"x".to_vec(),
            error: None,
        }
    );
    assert_eq!(
        frames(server.on_frame(Frame::Command {
            id: 7,
            name: "echo".into(),
            payload: b"y".to_vec(),
        })),
        vec![result]
    );
}

#[test]
fn handshake_and_ping_are_explicit() {
    let mut server = Server::new();
    assert!(matches!(
        frames(server.on_frame(Frame::Hello {
            version: PROTOCOL_VERSION + 1,
            client: "test".into(),
        }))
        .as_slice(),
        [Frame::Error { .. }]
    ));
    assert!(matches!(
        subscribe(&mut server, &["settings"], &[], 0).as_slice(),
        [Frame::Error { .. }]
    ));
    assert_eq!(hello(&mut server).len(), 1);
    assert_eq!(frames(server.on_frame(Frame::Ping)), vec![Frame::Pong]);
}

#[test]
fn a_ping_reaches_the_client_and_pong_is_ignored() {
    let mut client = Client::new();
    assert_eq!(client.on_frame(Frame::Ping), vec![ClientUpdate::Ping]);
    let mut server = Server::new();
    hello(&mut server);
    assert!(frames(server.on_frame(Frame::Pong)).is_empty());
}

#[test]
fn a_new_client_accepts_the_first_event_after_welcome() {
    let mut server = Server::new();
    server.events_mut().push("old", Vec::new());
    server.events_mut().push("old", Vec::new());
    let mut client = Client::new();
    for frame in hello(&mut server) {
        client.on_frame(frame);
    }
    for frame in frames(server.on_frame(client.subscribe(vec!["settings".into()]))) {
        client.on_frame(frame);
    }
    let seq = server.events_mut().push("new", b"payload".to_vec());
    assert_eq!(seq, 3);
    let frames = server.flush();
    assert_eq!(
        frames,
        vec![Frame::Event {
            seq: 3,
            kind: "new".into(),
            payload: b"payload".to_vec(),
        }]
    );
    let updates = client.on_frame(frames[0].clone());
    assert!(matches!(
        updates.as_slice(),
        [ClientUpdate::Event { seq: 3, .. }]
    ));
}

#[test]
fn a_new_client_does_not_replay_historical_events() {
    let mut server = Server::new();
    hello(&mut server);
    server.events_mut().push("notice", b"old".to_vec());
    server.events_mut().push("notice", b"older".to_vec());
    assert!(subscribe(&mut server, &["settings"], &[], 0).is_empty());
    server.events_mut().push("notice", b"new".to_vec());
    let frames = server.flush();
    assert!(matches!(frames.as_slice(), [Frame::Event { .. }]));
}

#[test]
fn events_follow_the_client_cursor() {
    let mut server = Server::new();
    hello(&mut server);
    let first = server.events_mut().push("notice", b"one".to_vec());
    let second = server.events_mut().push("notice", b"two".to_vec());
    assert_eq!(
        subscribe(&mut server, &["settings"], &[], first),
        vec![Frame::Event {
            seq: second,
            kind: "notice".into(),
            payload: b"two".to_vec(),
        }]
    );
}

#[test]
fn a_lost_changed_is_recovered_by_resync() {
    let mut server = Server::new();
    let mut client = Client::new();
    for frame in hello(&mut server) {
        client.on_frame(frame);
    }
    server.hub_mut().publish("settings", 1, b"one".to_vec());
    for frame in frames(server.on_frame(client.subscribe(vec!["settings".into()]))) {
        client.on_frame(frame);
    }
    assert_eq!(client.view().version("settings"), Some(1));

    server.hub_mut().publish("settings", 2, b"two".to_vec());
    assert_eq!(server.flush().len(), 1);

    for frame in frames(server.on_frame(client.resync_frame())) {
        client.on_frame(frame);
    }
    assert_eq!(client.view().version("settings"), Some(2));
}

#[test]
fn a_reconnecting_stale_client_gets_a_snapshot() {
    let mut server = Server::new();
    hello(&mut server);
    subscribe(&mut server, &["recording"], &[], 0);
    server.hub_mut().publish("recording", 1, b"idle".to_vec());
    server.flush();

    let mut second = Client::new();
    for frame in hello(&mut server) {
        second.on_frame(frame);
    }
    server
        .hub_mut()
        .publish("recording", 2, b"recording".to_vec());
    assert_eq!(
        subscribe(&mut server, &["recording"], &[("recording", 1)], 0),
        vec![Frame::Snapshot {
            store: "recording".into(),
            version: 2,
            payload: b"recording".to_vec(),
        }]
    );
}

#[test]
fn the_client_ignores_old_state_and_duplicate_events() {
    let mut client = Client::new();
    let state = Frame::Snapshot {
        store: "a".into(),
        version: 2,
        payload: Vec::new(),
    };
    assert_eq!(client.on_frame(state.clone()).len(), 1);
    assert!(client.on_frame(state).is_empty());
    let event = Frame::Event {
        seq: 1,
        kind: "x".into(),
        payload: Vec::new(),
    };
    assert_eq!(client.on_frame(event.clone()).len(), 1);
    assert!(client.on_frame(event).is_empty());
}

#[test]
fn the_client_detects_an_event_gap_and_resets_on_resync() {
    let mut client = Client::new();
    client.subscribe(vec!["a".into()]);
    client.on_frame(Frame::Event {
        seq: 1,
        kind: "x".into(),
        payload: Vec::new(),
    });
    assert_eq!(
        client.on_frame(Frame::Event {
            seq: 3,
            kind: "x".into(),
            payload: Vec::new(),
        }),
        vec![ClientUpdate::Gap]
    );
    client.on_frame(Frame::Snapshot {
        store: "a".into(),
        version: 4,
        payload: Vec::new(),
    });
    assert_eq!(
        client.on_frame(Frame::Resync {
            stores: vec!["a".into()],
            event_seq: 10,
        }),
        vec![ClientUpdate::Resync]
    );
    assert_eq!(client.view().version("a"), None);
    assert_eq!(client.event_cursor(), 10);
}

#[test]
fn the_client_correlates_results_and_tracks_pending() {
    let mut client = Client::new();
    client.command_frame(5, "echo", b"x".to_vec());
    assert!(client.pending().contains(&5));
    let updates = client.on_frame(Frame::Result {
        id: 5,
        ok: true,
        payload: b"x".to_vec(),
        error: None,
    });
    assert!(matches!(
        updates.as_slice(),
        [ClientUpdate::Result {
            id: 5,
            ok: true,
            ..
        }]
    ));
    assert!(!client.pending().contains(&5));
}

#[test]
fn hub_view_and_event_log_keep_their_invariants() {
    let mut hub = StateHub::new();
    assert!(hub.publish("s", 1, b"a".to_vec()));
    assert!(hub.publish("s", 2, b"b".to_vec()));
    assert!(!hub.publish("s", 1, b"old".to_vec()));

    let mut view = StateView::new();
    assert!(view.accept("s", 2));
    assert!(!view.accept("s", 2));

    let mut log = EventLog::new(2);
    log.push("a", Vec::new());
    log.push("b", Vec::new());
    log.push("c", Vec::new());
    let seen = log.since(0);
    assert!(seen.gap);
    assert_eq!(seen.events.len(), 2);
}
