mod dispatcher;

pub use dispatcher::{
    spawn_outbox_dispatcher, DeliveryMode, OutboxConsumer, OutboxDispatcherHandle,
    OutboxDispatcherSettings, OutboxProcessError,
};
