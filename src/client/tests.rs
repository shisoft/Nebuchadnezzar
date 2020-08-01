use crate::client;
use crate::client::transaction::TxnError;
use crate::ram::cell::*;
use crate::ram::schema::*;
use crate::ram::types;
use crate::ram::types::*;
use crate::server::*;
use crate::ram::tests::default_fields;
use parking_lot::Mutex;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::server;
use super::*;

#[tokio::test(threaded_scheduler)]
pub async fn general() {
    let server_group = "general_test";
    let server_addr = String::from("127.0.0.1:5400");
    let server = NebServer::new_from_opts(
        &ServerOptions {
            chunk_count: 1,
            memory_size: 16 * 1024 * 1024,
            backup_storage: None,
            wal_storage: None,
            services: vec![Service::Cell, Service::Transaction],
        },
        &server_addr,
        &server_group,
    ).await;
    let schema = Schema {
        id: 1,
        name: String::from("test"),
        key_field: None,
        str_key_field: None,
        fields: default_fields(),
        is_dynamic: false,
    };
    let client =
        Arc::new(client::AsyncClient::new(&server.rpc, &server.membership, &vec![server_addr], server_group).await.unwrap());
    let schema_id = client.new_schema(schema).await.unwrap().0;
    let mut data_map = Map::new();
    data_map.insert(&String::from("id"), Value::I64(100));
    data_map.insert(&String::from("score"), Value::U64(0));
    data_map.insert(&String::from("name"), Value::String(String::from("Jack")));
    let cell_1 = Cell::new_with_id(schema_id, &Id::rand(), Value::Map(data_map.clone()));
    client.write_cell(cell_1.clone()).await.unwrap().unwrap();
    client
        .read_cell(cell_1.clone().id())
        .await
        .unwrap()
        .unwrap();
    client
        .transaction(|ref mut _trans| {
          future::ready(Ok(())) // empty transaction
        })
        .await
        .unwrap();
    let should_aborted = client.transaction(|trans|trans.abort()).await;
    match should_aborted {
        Err(TxnError::Aborted(_)) => {}
        _ => panic!("{:?}", should_aborted),
    }

    // TODO: investigate dead lock
    //    client.transaction(|ref mut trans| {
    //        trans.write(&cell_1) // regular fail case
    //    }).err().unwrap();
    client
        .transaction(move |trans| {
            let empty_cell = Cell::new_with_id(schema_id, &Id::rand(), Value::Map(Map::new()));
            trans.write(empty_cell.to_owned()) // empty cell write should fail
        })
        .await
        .err()
        .unwrap();

    let cell_1_id = cell_1.id();
    let thread_count = 50;
    let mut futs: FuturesUnordered<_> = FuturesUnordered::new();
    for _ in 0..thread_count {
        let client = client.clone();
        futs.push(async move {
            client
                .transaction(async move |ref mut txn| {
                    let mut cell = txn.read(cell_1_id.to_owned()).await?.unwrap();
                    // WARNING: read_selected is subject to dirty read
                    let selected = txn
                        .read_selected(
                            cell_1_id.to_owned(),
                            types::key_hashes(&vec![String::from("score")]),
                        ).await?
                        .unwrap();
                    let mut score = *cell.data["score"].U64().unwrap();
                    assert_eq!(selected.first().unwrap().U64().unwrap(), &score);
                    score += 1;
                    let mut data = cell.data.Map().unwrap().clone();
                    data.insert(&String::from("score"), Value::U64(score));
                    cell.data = Value::Map(data);
                    txn.update(cell.to_owned()).await?;
                    let selected = txn
                        .read_selected(
                            cell_1_id.to_owned(),
                            types::key_hashes(&vec![String::from("score")]),
                        ).await?
                        .unwrap();
                    assert_eq!(selected[0].U64().unwrap(), &score);
  
                    let header = txn.head(cell.id()).await?.unwrap();
                    assert_eq!(header.id(), cell.id());
                    assert!(header.version > 1);
  
                    Ok(())
                })
                .await
                .unwrap()
        });
    }
    let _: Vec<_> = futs.collect().await;
    let cell_1_r = client.read_cell(cell_1.id()).await.unwrap().unwrap();
    assert_eq!(
        cell_1_r.data["score"].U64().unwrap(),
        &(thread_count as u64)
    );
}

#[tokio::test(threaded_scheduler)]
pub async fn multi_cell_update() {
    let server_group = "multi_cell_update_test";
    let server_addr = String::from("127.0.0.1:5401");
    let server = NebServer::new_from_opts(
        &ServerOptions {
            chunk_count: 1,
            memory_size: 16 * 1024 * 1024,
            backup_storage: None,
            wal_storage: None,
            services: vec![Service::Cell, Service::Transaction],
        },
        &server_addr,
        server_group,
    ).await;
    let schema = Schema {
        id: 1,
        name: String::from("test"),
        key_field: None,
        str_key_field: None,
        fields: default_fields(),
        is_dynamic: false,
    };
    let client =
        Arc::new(client::AsyncClient::new(&server.rpc, &server.membership, &vec![server_addr], server_group).await.unwrap());
    let thread_count = 100;
    let schema_id = schema.id;
    client.new_schema(schema).await.unwrap();
    let mut data_map = Map::new();
    data_map.insert(&String::from("id"), Value::I64(100));
    data_map.insert(&String::from("score"), Value::U64(0));
    data_map.insert(&String::from("name"), Value::String(String::from("Jack")));
    let cell_1 = Cell::new_with_id(schema_id, &Id::rand(), Value::Map(data_map.clone()));
    client.write_cell(cell_1.clone()).await.unwrap().unwrap();
    client.read_cell(cell_1.id()).await.unwrap().unwrap();
    let cell_1_id = cell_1.id();
    let mut cell_2 = cell_1.clone();
    cell_2.set_id(&Id::rand());
    client.write_cell(cell_2.clone()).await.unwrap().unwrap();
    client.read_cell(cell_2.id()).await.unwrap().unwrap();
    let cell_2_id = cell_2.id();
    let futs: FuturesUnordered<_> = FuturesUnordered::new(); 
    for _i in 0..thread_count {
        let client = client.clone();
        futs.push(
            async move {
                client
                .transaction(async move |txn| {
                    let mut score_1 = 0;
                    let mut score_2 = 0;
                    let mut cell_1 = txn.read(cell_1_id.to_owned()).await?.unwrap();
                    let mut cell_2 = txn.read(cell_2_id.to_owned()).await?.unwrap();
                    score_1 = *cell_1.data["score"].U64().unwrap();
                    score_2 = *cell_2.data["score"].U64().unwrap();
                    score_1 += 1;
                    score_2 += 1;
                    let mut data_1 = cell_1.data.Map().unwrap().clone();
                    data_1.insert(&String::from("score"), Value::U64(score_1));
                    cell_1.data = Value::Map(data_1);
                    let mut data_2 = cell_2.data.Map().unwrap().clone();
                    data_2.insert(&String::from("score"), Value::U64(score_2));
                    cell_2.data = Value::Map(data_2);
                    txn.update(cell_1.to_owned()).await?;
                    txn.update(cell_2.to_owned()).await?;
                    Ok(())
                })
                .await
                .unwrap();
            }
        );
    }
    let _: Vec<_> = futs.collect().await;
    let cell_1_r = client.read_cell(cell_1_id).await.unwrap().unwrap();
    let cell_2_r = client.read_cell(cell_2_id).await.unwrap().unwrap();
    let cell_1_score = cell_1_r.data["score"].U64().unwrap();
    let cell_2_score = cell_2_r.data["score"].U64().unwrap();
    assert_eq!(cell_1_score + cell_2_score, (thread_count * 2) as u64);
}

#[tokio::test(threaded_scheduler)]
pub async fn write_skew() {
    let server_group = "write_skew_test";
    let server_addr = String::from("127.0.0.1:5402");
    let server = NebServer::new_from_opts(
        &ServerOptions {
            chunk_count: 1,
            memory_size: 16 * 1024 * 1024,
            backup_storage: None,
            wal_storage: None,
            services: vec![Service::Cell, Service::Transaction],
        },
        &server_addr,
        server_group,
    ).await;
    let schema = Schema {
        id: 1,
        name: String::from("test"),
        key_field: None,
        str_key_field: None,
        fields: default_fields(),
        is_dynamic: false,
    };
    let client =
        Arc::new(client::AsyncClient::new(&server.rpc, &server.membership, &vec![server_addr], server_group).await.unwrap());
    let schema_id = client.new_schema(schema).await.unwrap().0;
    let mut data_map = Map::new();
    data_map.insert(&String::from("id"), Value::I64(100));
    data_map.insert(&String::from("score"), Value::U64(0));
    data_map.insert(&String::from("name"), Value::String(String::from("Jack")));
    let cell_1 = Cell::new_with_id(schema_id, &Id::rand(), Value::Map(data_map.clone()));
    client.write_cell(cell_1.clone()).await.unwrap().unwrap();
    client.read_cell(cell_1.id()).await.unwrap().unwrap();
    let cell_1_id = cell_1.id();
    let client_c1 = client.clone();
    let skew_tried = Arc::new(Mutex::new(0usize));
    let normal_tried = Arc::new(Mutex::new(0usize));

    let skew_tried_c = skew_tried.clone();
    let normal_tried_c = normal_tried.clone();

    let t1 = tokio::spawn(async {
        client_c1
            .transaction(async move |ref mut txn| {
                *skew_tried_c.lock() += 1;
                let mut cell_1 = txn.read(cell_1_id.to_owned()).await?.unwrap();
                let mut score_1 = *cell_1.data["score"].U64().unwrap();
                thread::sleep(Duration::new(2, 0)); // wait 2 secs to let late write occur
                score_1 += 1;
                let mut data_1 = cell_1.data.Map().unwrap().clone();
                data_1.insert(&String::from("score"), Value::U64(score_1));
                cell_1.data = Value::Map(data_1);
                txn.update(cell_1.to_owned()).await?;
                Ok(())
            })
            .await;
    });
    let client_c2 = client.clone();
    let t2 = tokio::spawn(async {
        client_c2
            .transaction(async move |ref mut txn| {
                thread::sleep(Duration::new(1, 0));
                *normal_tried_c.lock() += 1;
                let mut cell_1 = txn.read(cell_1_id.to_owned()).await?.unwrap();
                let mut score_1 = *cell_1.data["score"].U64().unwrap();
                score_1 += 1;
                let mut data_1 = cell_1.data.Map().unwrap().clone();
                data_1.insert(&String::from("score"), Value::U64(score_1));
                cell_1.data = Value::Map(data_1);
                txn.update(cell_1.to_owned()).await?;
                Ok(())
            })
            .await;
    });
    t2.await;
    t1.await;
    let cell_1_r = client.read_cell(cell_1_id).await.unwrap().unwrap();
    let cell_1_score = *cell_1_r.data["score"].U64().unwrap();
    assert_eq!(cell_1_score, 2);
    //    assert_eq!(*skew_tried.lock(), 2);
    //    assert_eq!(*normal_tried.lock(), 1);
    println!(
        "Skew tried {}, normal tried {}",
        *skew_tried.lock(),
        *normal_tried.lock()
    );
}

#[tokio::test(threaded_scheduler)]
pub async fn server_isolation() {
    let server_1_group = "server_isolation_test_1";
    let server_2_group = "server_isolation_test_2";
    let server_address_1 = "127.0.0.1:5403";
    let server_address_2 = "127.0.0.1:5404";

    let server_1 = NebServer::new_from_opts(
        &ServerOptions {
            chunk_count: 1,
            memory_size: 16 * 1024 * 1024,
            backup_storage: None,
            wal_storage: None,
            services: vec![Service::Cell, Service::Transaction],
        },
        server_address_1,
        server_1_group,
    ).await;
    let client1 = Arc::new(
        client::AsyncClient::new(
            &server_1.rpc,
            &server_1.membership,
            &vec![server_address_1.to_string()],
            server_1_group,
        )
        .await
        .unwrap(),
    );

    let server_2 = NebServer::new_from_opts(
        &ServerOptions {
            chunk_count: 1,
            memory_size: 16 * 1024 * 1024,
            backup_storage: None,
            wal_storage: None,
            services: vec![Service::Cell, Service::Transaction],
        },
        server_address_2,
        server_2_group,
    ).await;
    let client2 = Arc::new(
        client::AsyncClient::new(
            &server_2.rpc,
            &server_2.membership,
            &vec![server_address_2.to_string()],
            server_2_group,
        )
        .await
        .unwrap(),
    );

    let schema1 = Schema {
        id: 1,
        name: String::from("test"),
        key_field: None,
        str_key_field: None,
        fields: default_fields(),
        is_dynamic: false,
    };
    let schema2 = Schema {
        id: 1,
        name: String::from("test"),
        key_field: None,
        str_key_field: None,
        fields: Field::new(
            &String::from("*"),
            0,
            false,
            false,
            Some(vec![
                Field::new(&String::from("-id"), 6, false, false, None, vec![]),
                Field::new(&String::from("-name"), 20, false, false, None, vec![]),
                Field::new(&String::from("-score"), 10, false, false, None, vec![]),
            ]),
            vec![],
        ),
        is_dynamic: false,
    };

    client1
        .new_schema_with_id(schema1.clone())
        .await
        .unwrap()
        .unwrap();
    client2
        .new_schema_with_id(schema2.clone())
        .await
        .unwrap()
        .unwrap();

    // println!("{:?}", client1.schema_client.get(&schema1.id));

    let schema_1_got: Schema = client1
        .get_all_schema()
        .await
        .unwrap()
        .first()
        .unwrap()
        .clone();
    assert_eq!(schema_1_got.id, 1);
    let schema_1_fields = schema1.fields;
    assert_eq!(
        schema_1_fields
            .clone()
            .sub_fields
            .unwrap()
            .first()
            .unwrap()
            .name,
        default_fields().sub_fields.unwrap().first().unwrap().name
    );
    assert_eq!(
        schema_1_fields
            .clone()
            .sub_fields
            .unwrap()
            .get(1)
            .unwrap()
            .name,
        default_fields().sub_fields.unwrap().get(1).unwrap().name
    );
    assert_eq!(
        schema_1_fields
            .clone()
            .sub_fields
            .unwrap()
            .get(2)
            .unwrap()
            .name,
        default_fields().sub_fields.unwrap().get(2).unwrap().name
    );

    let schema_2_got: Schema = client2
        .get_all_schema()
        .await
        .unwrap()
        .first()
        .unwrap()
        .clone();
    assert_eq!(schema_2_got.id, 1);
    let schema_2_fields = schema2.fields;
    assert_eq!(
        schema_2_fields
            .clone()
            .sub_fields
            .unwrap()
            .first()
            .unwrap()
            .name,
        "-id"
    );
    assert_eq!(
        schema_2_fields
            .clone()
            .sub_fields
            .unwrap()
            .get(1)
            .unwrap()
            .name,
        "-name"
    );
    assert_eq!(
        schema_2_fields
            .clone()
            .sub_fields
            .unwrap()
            .get(2)
            .unwrap()
            .name,
        "-score"
    );
}
