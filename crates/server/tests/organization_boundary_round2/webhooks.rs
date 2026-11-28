//! A scoped background write retains its board; webhook destinations are global.

use std::collections::BTreeSet;
use std::time::Duration;

use axum::extract::State;
use axum::{Json, Router};
use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_model::{Block, BlockType, new_id};
use fvoci_server::app::{App, ChangeNotification};
use fvoci_store::DbType;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use super::db_type_of_the_tests;
use super::media::Scene;

async fn receive(
    State(sender): State<mpsc::Sender<Value>>,
    Json(block): Json<Value>,
) -> axum::http::StatusCode {
    sender.send(block).await.expect("the test receiver is open");
    axum::http::StatusCode::NO_CONTENT
}

#[tokio::test]
async fn a_global_webhook_receives_each_scoped_background_change_with_its_own_board() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let scene = Scene::new(None).await;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("an isolated receiver");
    let address = listener.local_addr().expect("receiver address");
    let (sender, mut received) = mpsc::channel(4);
    let router = Router::new()
        .route("/hook", axum::routing::post(receive))
        .with_state(sender);
    let (stop, stopped) = oneshot::channel();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .expect("the receiver serves");
    });
    let mut config = scene.ts.app.config().clone();
    config.block_change_webhooks = vec![format!("http://{address}/hook")];
    let (settings, _) = config.check().expect("the configuration with the hook");
    let a = App::new(
        settings,
        scene.ts.app.store().clone(),
        scene.ts.app.files().clone(),
    )
    .expect("the configured global hook");
    let b = a.scoped(scene.b_app.store().tenant());
    for (app, board) in [(&a, &scene.a), (&b, &scene.b)] {
        let mut block = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            title: board.title.clone(),
            ..Block::default()
        };
        app.insert_block_unchecked(&mut block, &scene.user, ChangeNotification::Suppress)
            .await
            .expect("an authorized scoped change");
    }
    assert!(
        a.finish_background(Duration::from_secs(5)).await,
        "both background notifications finish"
    );
    let mut boards = BTreeSet::new();
    for _ in 0..2 {
        let block = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("a webhook arrives")
            .expect("the receiver remains open");
        boards.insert(
            block["containerId"]
                .as_str()
                .expect("a board ID")
                .to_owned(),
        );
    }
    assert_eq!(
        boards,
        [scene.a.id.clone(), scene.b.id.clone()]
            .into_iter()
            .collect(),
        "the global installation hook receives both organizations with the correct board"
    );
    let _ = stop.send(());
    serving.await.expect("receiver shutdown");
    scene.ts.teardown().await;
}

#[tokio::test]
async fn a_second_orgs_read_token_can_subscribe_to_its_card_updates() {
    let (ts, app, user, board, card) = if db_type_of_the_tests() == DbType::Postgres {
        let scene = Scene::new(None).await;
        (scene.ts, scene.b_app, scene.user, scene.b, scene.card_b)
    } else {
        let ts = super::common::Setup::local_mode()
            .start()
            .await
            .init_basic()
            .await;
        let user = ts.client.me().await.id;
        let board = ts
            .create_board(
                super::common::ROOT_WORKSPACE_ID,
                fvoci_model::ContainerVisibility::Private,
            )
            .await;
        let mut card = Block {
            id: new_id(),
            container_id: board.id.clone(),
            block_type: BlockType::Card,
            ..Block::default()
        };
        ts.app
            .store()
            .insert_block(&mut card, &user)
            .await
            .expect("card stored");
        let app = ts.app.clone();
        (ts, app, user, board, card)
    };
    let token = app
        .set_public_link(&board.id, fvoci_model::PublicLinkChange::On, &user)
        .await
        .expect("B is shared")
        .token;
    ts.anonymous()
        .get(&format!("/shared/{}?readToken={token}", board.id))
        .await
        .check_ok();
    let mut socket = ts.anonymous().as_browser().connect_websocket().await;
    socket
        .send(Message::Text(
            serde_json::json!({
                "type": "subscribeBlocks", "workspaceId": board.workspace_id,
                "blockIds": [card.id], "readToken": token
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("B's card subscription is sent");
    // The subscription is registered before the change it is to be told of.
    super::common::carried_out(app.ws(), &mut socket).await;
    app.ws()
        .broadcast_block_change(&board.workspace_id, &card)
        .await;
    let update = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(message) = socket.next().await {
            if let Ok(Message::Text(text)) = message {
                let value: Value = serde_json::from_str(&text).expect("a JSON update");
                if value["block"]["id"] == card.id {
                    return Some(value);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = socket.close(None).await;
    ts.teardown().await;
    assert!(
        update.is_some(),
        "B's valid public link loads over HTTP but its websocket subscription receives no update"
    );
    assert_eq!(
        update.expect("the update was checked")["block"]["createdBy"],
        "",
        "the public update hides people"
    );
}
