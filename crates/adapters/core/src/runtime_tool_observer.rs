//! Shared observer wrapper for runtime tool notifications.
//!
//! Adapter transports own payload delivery, but they should not fork observer
//! forwarding mechanics.

use crate::runtime_tool_notifications::RuntimeToolNotification;
use std::sync::Arc;
use synapse_observability::traits::{ObserverEvent, ObserverMetric};
use synapse_observability::Observer;

pub(crate) trait RuntimeToolNotificationHandler: Send + Sync {
    fn notify(&self, notification: RuntimeToolNotification);
}

pub(crate) struct RuntimeToolNotifyObserver<H> {
    inner: Arc<dyn Observer>,
    handler: H,
    name: &'static str,
}

impl<H> RuntimeToolNotifyObserver<H>
where
    H: RuntimeToolNotificationHandler,
{
    pub(crate) fn new(inner: Arc<dyn Observer>, handler: H, name: &'static str) -> Self {
        Self {
            inner,
            handler,
            name,
        }
    }
}

impl<H> Observer for RuntimeToolNotifyObserver<H>
where
    H: RuntimeToolNotificationHandler + 'static,
{
    fn record_event(&self, event: &ObserverEvent) {
        if let Some(notification) = RuntimeToolNotification::from_observer_event(event) {
            self.handler.notify(notification);
        }
        self.inner.record_event(event);
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        self.inner.record_metric(metric);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn name(&self) -> &str {
        self.name
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
