//! Outbound intent bus — connects gateway (producer) to channels (consumer).
//!
//! The daemon creates a bus, clones the sender into the gateway, and hands
//! the receiver to the channels subsystem (or a dedicated relay task).

use super::domain::channel::OutboundIntent;
use tokio::sync::mpsc;

/// Sender half — cloneable, passed to gateway / inbox processor.
#[derive(Clone, Debug)]
pub struct OutboundIntentSender {
    tx: mpsc::UnboundedSender<OutboundIntent>,
}

impl OutboundIntentSender {
    /// Emit an outbound intent.  Returns `false` if the receiver was dropped.
    pub fn send(&self, intent: OutboundIntent) -> bool {
        self.tx.send(intent).is_ok()
    }
}

/// Receiver half — **not** cloneable.  Consumed by the relay task.
#[derive(Debug)]
pub struct OutboundIntentReceiver {
    rx: mpsc::UnboundedReceiver<OutboundIntent>,
}

impl OutboundIntentReceiver {
    /// Wait for the next intent.  Returns `None` when all senders are dropped.
    pub async fn recv(&mut self) -> Option<OutboundIntent> {
        self.rx.recv().await
    }
}

/// Create a linked sender/receiver pair.
pub fn outbound_intent_bus() -> (OutboundIntentSender, OutboundIntentReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (OutboundIntentSender { tx }, OutboundIntentReceiver { rx })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_and_receive() {
        let (tx, mut rx) = outbound_intent_bus();
        let intent = OutboundIntent::notify("telegram", "123", "hi".into());
        assert!(tx.send(intent));
        let received = rx.recv().await.unwrap();
        assert_eq!(received.target_channel, "telegram");
        assert_eq!(received.content.as_text(), "hi");
    }

    #[tokio::test]
    async fn receiver_returns_none_when_sender_dropped() {
        let (tx, mut rx) = outbound_intent_bus();
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn sender_clone_works() {
        let (tx, mut rx) = outbound_intent_bus();
        let tx2 = tx.clone();
        tx.send(OutboundIntent::notify("a", "1", "from tx".into()));
        tx2.send(OutboundIntent::notify("b", "2", "from tx2".into()));
        let first = rx.recv().await.unwrap();
        let second = rx.recv().await.unwrap();
        assert_eq!(first.target_channel, "a");
        assert_eq!(second.target_channel, "b");
    }
}
