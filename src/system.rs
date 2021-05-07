use std::{any::Any, collections::HashMap, sync::Arc};

use tokio::sync::RwLock;

use crate::{ActorError, ActorPath, actor::{Actor, ActorRef, runner::ActorRunner}, bus::{EventBus, EventConsumer}};

pub trait SystemEvent: Clone + Send + Sync + 'static {}

#[derive(Clone)]
pub struct ActorSystem<E: SystemEvent> {
    name: String,
    actors: Arc<RwLock<HashMap<ActorPath, Box<dyn Any + Send + Sync + 'static>>>>,
    bus: EventBus<E>
}

impl<E: SystemEvent> ActorSystem<E> {

    pub fn get_name(&self) -> &str {
        &self.name
    }

    pub fn publish(&self, event: E) {
        self.bus.send(event).unwrap_or_else(|error| {
            log::error!("Failed to publish event! {}", error.to_string());
            0
        });
    }

    pub fn events(&self) -> EventConsumer<E> {
        self.bus.subscribe()
    }

    pub async fn get_actor<A: Actor>(&self, path: &ActorPath) -> Option<ActorRef<A, E>> {
        let actors = self.actors.read().await;
        actors.get(path).and_then(|any| {
            any.downcast_ref::<ActorRef<A, E>>().cloned()
        })
    }

    pub async fn create_actor<A: Actor>(&self, path: ActorPath, actor: A) -> Result<ActorRef<A, E>, ActorError> {

        let mut actors = self.actors.write().await;
        if actors.contains_key(&path) {
            return Err(ActorError::Create( format!("Actor path '{}' already exists.", &path) ))
        }

        let system = self.clone();
        let (mut runner, actor_ref) = ActorRunner::create(path, actor);
        tokio::spawn( async move {
            runner.start(system).await;
        });

        let path = actor_ref.get_path().clone();
        let any = Box::new(actor_ref.clone());

        actors.insert(path, any);

        Ok(actor_ref)
    }

    pub async fn stop_actor(&self, path: &ActorPath) {
        let mut actors = self.actors.write().await;
        actors.remove(path);
    }

    pub fn new(name: &str, bus: EventBus<E>) -> Self {
        let name = name.to_string();
        let actors = Arc::new(RwLock::new(HashMap::new()));
        ActorSystem { name, actors, bus }
    }
}

#[cfg(test)]
mod tests {

    use async_trait::async_trait;
    use crate::actor::{Actor, ActorContext, Handler, Message};

    use super::*;

    #[derive(Clone, Debug)]
    struct TestEvent(String);

    impl SystemEvent for TestEvent {}

    #[derive(Clone)]
    struct TestActor {
        counter: usize
    }

    impl Actor for TestActor {}

    #[derive(Clone, Debug)]
    struct TestMessage(usize);

    impl Message for TestMessage {
        type Response = usize;
    }

    impl SystemEvent for TestMessage {}

    #[async_trait]
    impl Handler<TestMessage, TestEvent> for TestActor {
        async fn handle(&mut self, msg: TestMessage, ctx: &mut ActorContext<TestEvent>) -> usize {
            log::debug!("received message! {:?}", &msg);
            self.counter += 1;
            log::debug!("counter is now {}", &self.counter);
            log::debug!("actor on system {}", ctx.system.get_name());
            ctx.system.publish(TestEvent("Message received!".to_string()));
            self.counter
        }
    }

    #[derive(Clone)]
    struct OtherActor {
        message: String
    }

    impl Actor for OtherActor {}

    #[derive(Clone, Debug)]
    struct OtherMessage(String);

    impl Message for OtherMessage {
        type Response = String;
    }

    #[async_trait]
    impl Handler<OtherMessage, TestEvent> for OtherActor {
        async fn handle(&mut self, msg: OtherMessage, ctx: &mut ActorContext<TestEvent>) -> String {
            log::debug!("OtherActor received message! {:?}", &msg);
            log::debug!("original message is {}", &self.message);
            self.message = msg.0;
            log::debug!("message is now {}", &self.message);
            log::debug!("actor on system {}", ctx.system.get_name());
            ctx.system.publish(TestEvent("Received message!".to_string()));
            self.message.clone()
        }
    }

    #[tokio::test]
    async fn actor_create() {
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "trace");
        }
        let _ = env_logger::builder().is_test(true).try_init();

        let actor = TestActor { counter: 0 };
        let msg = TestMessage(10);

        let bus = EventBus::<TestEvent>::new(1000);
        let system = ActorSystem::new("test", bus);
        let path = ActorPath::from("/some/actor");
        let mut actor_ref = system.create_actor(path, actor).await.unwrap();
        let result = actor_ref.ask(msg).await.unwrap();

        assert_eq!(result, 1);
    }

    #[tokio::test]
    async fn actor_stop() {
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "trace");
        }
        let _ = env_logger::builder().is_test(true).try_init();

        let actor = TestActor { counter: 0 };
        let msg = TestMessage(10);

        let bus = EventBus::<TestEvent>::new(1000);
        let system = ActorSystem::new("test", bus);

        {
            let path = ActorPath::from("/some/actor");
            let mut actor_ref = system.create_actor(path, actor).await.unwrap();
            let result = actor_ref.ask(msg).await.unwrap();

            assert_eq!(result, 1);

            system.stop_actor(actor_ref.get_path()).await;
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }

    #[tokio::test]
    async fn actor_events() {
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "trace");
        }
        let _ = env_logger::builder().is_test(true).try_init();

        let actor = TestActor { counter: 0 };
        let msg = TestMessage(10);

        let bus = EventBus::<TestEvent>::new(1000);
        let system = ActorSystem::new("test", bus);
        let path = ActorPath::from("/some/actor");
        let mut actor_ref = system.create_actor(path, actor).await.unwrap();

        let mut events = system.events();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => println!("Received event! {:?}", event),
                    Err(err) => println!("Error receivng event!!! {:?}", err)
                }
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let result = actor_ref.ask(msg).await.unwrap();

        assert_eq!(result, 1);
    }

    #[tokio::test]
    async fn actor_get() {
        if std::env::var("RUST_LOG").is_err() {
            std::env::set_var("RUST_LOG", "trace");
        }
        let _ = env_logger::builder().is_test(true).try_init();

        let actor = TestActor { counter: 0 };

        let bus = EventBus::<TestEvent>::new(1000);
        let system = ActorSystem::new("test", bus);
        let path = ActorPath::from("/some/actor");
        let original = system.create_actor(path, actor).await.unwrap();

        if let Some(mut actor_ref) = system.get_actor::<TestActor>(original.get_path()).await {
            let msg = TestMessage(10);
            let result = actor_ref.ask(msg).await.unwrap();
            assert_eq!(result, 1);
        } else {
            panic!("It should have retrieved the actor!")
        }

        if let Some(mut actor_ref) = system.get_actor::<OtherActor>(original.get_path()).await {
            let msg = OtherMessage("Hello world!".to_string());
            let result = actor_ref.ask(msg).await.unwrap();
            println!("Result is: {}", result);
            panic!("It should not go here!");
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}