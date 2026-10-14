//! One terminal turn: capture requested keyframes, drain PTYs, then enqueue deltas.
//!
//! Pending backend bytes must not enter a snapshot and also be replayed as its
//! subsequent Output. The exchange owns that boundary, not its caller. Snapshots
//! and encoding stay real in tests; only the byte source and local clients vary.

use std::collections::{HashMap, VecDeque};

use cas_mux::{MuxEvent, Pane};
use futures_util::{FutureExt, SinkExt};

use super::loop_watchdog::{LoopPhase, LoopProgress};
use super::ws_client::{commander_epoch, ws_encode};
use crate::ui::factory::daemon::{FactoryDaemon, WsConnection};
use crate::ui::factory::protocol::DaemonMessage;

#[derive(Default)]
pub(in crate::ui::factory::daemon) struct TerminalExchange {
    requests: VecDeque<(usize, String)>,
}

/// The daemon's terminal resources. Draining updates the panes' real terminal
/// state; observing preserves buffering, GUI/relay delivery and exit handling.
/// Neither adapter chooses the snapshot/drain/frame ordering.
trait TerminalSession {
    fn pane(&self, pane_id: &str) -> Option<&Pane>;
    fn clients(&mut self) -> &mut HashMap<usize, WsConnection>;
    fn drain(&mut self) -> (usize, Vec<MuxEvent>);
    async fn observe_event(&mut self, event: MuxEvent);
}

impl TerminalExchange {
    pub(super) fn request(&mut self, client_id: usize, pane_id: String) {
        self.requests.push_back((client_id, pane_id));
    }

    async fn advance(&mut self, session: &mut impl TerminalSession) -> usize {
        self.capture_pending(session);
        let (bytes_processed, events) = session.drain();
        for event in events {
            let output = match &event {
                MuxEvent::PaneOutput { pane_id, data }
                    if !data.is_empty() && !session.clients().is_empty() =>
                {
                    Some(DaemonMessage::Output {
                        pane_id: pane_id.clone(),
                        data: data.clone(),
                    })
                }
                _ => None,
            };
            // Keep each event's old buffer/GUI/relay/exit effects before its WS
            // delta. Enqueueing all deltas first would reorder them with exits.
            session.observe_event(event).await;
            if let Some(frame) = output.as_ref().and_then(ws_encode) {
                for client in session.clients().values_mut() {
                    // Same deferred flush/backpressure policy as ws_broadcast.
                    let _ = client.sink.feed(frame.clone()).now_or_never();
                }
            }
        }
        bytes_processed
    }

    fn capture_pending(&mut self, session: &mut impl TerminalSession) {
        while let Some((client_id, pane_id)) = self.requests.pop_front() {
            if !session.clients().contains_key(&client_id) {
                continue;
            }
            let keyframe = session.pane(&pane_id).and_then(|pane| {
                let snapshot = pane.get_full_snapshot().ok()?;
                Some(DaemonMessage::PaneKeyframe {
                    pane_id,
                    epoch: commander_epoch(),
                    seq: 0,
                    cols: snapshot.cols,
                    rows: snapshot.rows,
                    ansi: super::relay::snapshot_to_ansi(&snapshot, pane.is_in_alt_screen()),
                })
            });
            if let Some(frame) = keyframe.as_ref().and_then(ws_encode)
                && let Some(client) = session.clients().get_mut(&client_id)
            {
                let _ = client.sink.feed(frame).now_or_never();
            }
        }
    }
}

struct DaemonTerminals<'a> {
    daemon: &'a mut FactoryDaemon,
    progress: &'a LoopProgress,
}

impl TerminalSession for DaemonTerminals<'_> {
    fn pane(&self, pane_id: &str) -> Option<&Pane> {
        self.daemon.app.mux.get(pane_id)
    }

    fn clients(&mut self) -> &mut HashMap<usize, WsConnection> {
        &mut self.daemon.ws_clients
    }

    fn drain(&mut self) -> (usize, Vec<MuxEvent>) {
        self.progress.enter(LoopPhase::PtyOutput);
        self.daemon.app.mux.poll_batch()
    }

    async fn observe_event(&mut self, event: MuxEvent) {
        self.daemon.handle_mux_event(event).await;
    }
}

impl FactoryDaemon {
    pub(super) async fn exchange_terminal(&mut self, progress: &LoopProgress) -> usize {
        let mut exchange = std::mem::take(&mut self.terminal_exchange);
        let bytes = exchange
            .advance(&mut DaemonTerminals {
                daemon: self,
                progress,
            })
            .await;
        self.terminal_exchange = exchange;
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_mux::Mux;
    use futures_util::StreamExt;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::{Duration, timeout};
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::{WebSocketStream, accept_async, client_async};

    /// Only the PTY byte source is substituted. Pane, snapshot capture, ANSI
    /// replay, exchange, wire encoding and WebSocket delivery are production.
    struct QueuedTerminals {
        mux: Mux,
        clients: HashMap<usize, WsConnection>,
        pending: VecDeque<Vec<u8>>,
        observed: Vec<MuxEvent>,
    }

    impl TerminalSession for QueuedTerminals {
        fn pane(&self, pane_id: &str) -> Option<&Pane> {
            self.mux.get(pane_id)
        }

        fn clients(&mut self) -> &mut HashMap<usize, WsConnection> {
            &mut self.clients
        }

        fn drain(&mut self) -> (usize, Vec<MuxEvent>) {
            let (mut bytes, mut events) = self.mux.poll_batch();
            let data: Vec<u8> = self.pending.drain(..).flatten().collect();
            if !data.is_empty() {
                self.mux.get_mut("pane").unwrap().feed(&data).unwrap();
                bytes += data.len();
                events.push(MuxEvent::PaneOutput {
                    pane_id: "pane".into(),
                    data,
                });
            }
            (bytes, events)
        }

        async fn observe_event(&mut self, event: MuxEvent) {
            self.observed.push(event);
        }
    }

    async fn local_client() -> (WsConnection, WebSocketStream<TcpStream>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (server, client) = timeout(Duration::from_secs(2), async {
            tokio::join!(
                async {
                    let (socket, _) = listener.accept().await.unwrap();
                    accept_async(socket).await.unwrap()
                },
                async {
                    let socket = TcpStream::connect(addr).await.unwrap();
                    client_async(format!("ws://{addr}"), socket)
                        .await
                        .unwrap()
                        .0
                }
            )
        })
        .await
        .expect("local WebSocket handshake timed out");
        let (sink, stream) = server.split();
        (
            WsConnection {
                sink,
                stream,
                pane_sizes: HashMap::new(),
                relay_trusted: false,
            },
            client,
        )
    }

    async fn receive(client: &mut WebSocketStream<TcpStream>) -> DaemonMessage {
        let frame = timeout(Duration::from_secs(2), client.next())
            .await
            .expect("terminal frame was not delivered")
            .expect("local client closed")
            .expect("local WebSocket read failed");
        let WsMessage::Binary(bytes) = frame else {
            panic!("daemon terminal messages must be binary JSON frames: {frame:?}");
        };
        serde_json::from_slice(&bytes).expect("invalid daemon wire message")
    }

    async fn flush(terminals: &mut QueuedTerminals) {
        for connection in terminals.clients.values_mut() {
            timeout(Duration::from_secs(2), connection.sink.flush())
                .await
                .expect("local frame flush timed out")
                .unwrap();
        }
    }

    // A later wire marker proves the preceding turn emitted no duplicates,
    // without relying on a short timeout to assert the absence of frames.
    async fn delivery_barrier(terminals: &mut QueuedTerminals) {
        for connection in terminals.clients.values_mut() {
            let frame = ws_encode(&DaemonMessage::Pong).unwrap();
            timeout(Duration::from_secs(2), connection.sink.feed(frame))
                .await
                .expect("local barrier enqueue timed out")
                .unwrap();
        }
        flush(terminals).await;
    }

    #[tokio::test]
    async fn delivered_keyframe_precedes_pending_bytes_and_reconstructs_terminal() {
        for alternate_screen in [false, true] {
            let (connection, mut client) = local_client().await;
            let (watcher_connection, mut watcher) = local_client().await;
            let mut pane = Pane::director("pane", 4, 40).unwrap();
            if alternate_screen {
                pane.feed(b"\x1b[?1049h").unwrap();
            }
            pane.feed(b"before \x1b[38;2;210;80;60mred\x1b[0m").unwrap();
            let before_drain = pane.get_full_snapshot().unwrap();
            let chunks = VecDeque::from([
                b" +\x1b[38;2;40;180;70mgreen".to_vec(),
                b"\x1b[0m\r\nnext".to_vec(),
            ]);
            let queued_bytes: Vec<u8> = chunks.iter().flatten().copied().collect();
            let mut mux = Mux::new(4, 40);
            mux.add_pane(pane);
            let mut terminals = QueuedTerminals {
                mux,
                clients: HashMap::from([(1, connection), (2, watcher_connection)]),
                pending: chunks,
                observed: Vec::new(),
            };
            let mut exchange = TerminalExchange::default();
            exchange.request(1, "pane".into());

            let bytes = exchange.advance(&mut terminals).await;
            flush(&mut terminals).await;
            assert_eq!(bytes, queued_bytes.len(), "PTY activity accounting changed");
            let DaemonMessage::PaneKeyframe {
                pane_id,
                epoch,
                seq,
                cols,
                rows,
                ansi,
            } = receive(&mut client).await
            else {
                panic!("first delivered frame must be the requested keyframe");
            };
            assert_eq!(pane_id, "pane");
            assert_eq!(epoch, commander_epoch());
            assert_eq!((seq, rows, cols), (0, 4, 40));

            let mut reconstructed = Pane::director("client", rows, cols).unwrap();
            reconstructed.feed(b"old client contents").unwrap();
            reconstructed.feed(&ansi).unwrap();
            assert_eq!(reconstructed.is_in_alt_screen(), alternate_screen);
            // This assertion rejects a drain-before-capture mutation even if
            // that mutation still delivers a keyframe before its Output frame.
            assert_eq!(
                reconstructed.get_full_snapshot().unwrap(),
                before_drain,
                "keyframe included bytes still pending at the exchange boundary"
            );

            let DaemonMessage::Output { pane_id, data } = receive(&mut client).await else {
                panic!("pending bytes must arrive after the keyframe as Output");
            };
            assert_eq!(pane_id, "pane");
            assert_eq!(data, queued_bytes);
            reconstructed.feed(&data).unwrap();
            assert_eq!(
                reconstructed.get_full_snapshot().unwrap(),
                terminals
                    .mux
                    .get("pane")
                    .unwrap()
                    .get_full_snapshot()
                    .unwrap(),
                "wire replay must preserve cells, styles and cursor"
            );
            // Keyframes are client-specific; deltas still reach every viewer.
            let DaemonMessage::Output { pane_id, data } = receive(&mut watcher).await else {
                panic!("unrequesting viewer must receive Output without a keyframe");
            };
            assert_eq!(pane_id, "pane");
            assert_eq!(data, queued_bytes);
            let observed_bytes: Vec<u8> = terminals
                .observed
                .iter()
                .filter_map(|event| match event {
                    MuxEvent::PaneOutput { data, .. } => Some(data.as_slice()),
                    _ => None,
                })
                .flatten()
                .copied()
                .collect();
            assert_eq!(observed_bytes, queued_bytes);

            assert_eq!(exchange.advance(&mut terminals).await, 0);
            delivery_barrier(&mut terminals).await;
            assert!(
                matches!(receive(&mut client).await, DaemonMessage::Pong),
                "consumed request or pending bytes were delivered twice"
            );
            assert!(matches!(receive(&mut watcher).await, DaemonMessage::Pong));
        }
    }

    #[tokio::test]
    async fn missing_pane_or_disconnected_request_does_not_suppress_live_output() {
        let (connection, mut client) = local_client().await;
        let mut mux = Mux::new(4, 40);
        mux.add_pane(Pane::director("pane", 4, 40).unwrap());
        let mut terminals = QueuedTerminals {
            mux,
            clients: HashMap::from([(1, connection)]),
            pending: VecDeque::from([b"live".to_vec()]),
            observed: Vec::new(),
        };
        let mut exchange = TerminalExchange::default();
        exchange.request(1, "missing".into());
        exchange.request(99, "pane".into());
        assert_eq!(exchange.advance(&mut terminals).await, 4);
        flush(&mut terminals).await;
        let DaemonMessage::Output { pane_id, data } = receive(&mut client).await else {
            panic!("invalid keyframe requests must not interrupt live output");
        };
        assert_eq!(pane_id, "pane");
        assert_eq!(data, b"live");
        assert_eq!(exchange.advance(&mut terminals).await, 0);
        delivery_barrier(&mut terminals).await;
        assert!(matches!(receive(&mut client).await, DaemonMessage::Pong));
    }
}
