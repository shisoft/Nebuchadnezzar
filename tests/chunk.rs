use neb::ram::cell;
use neb::ram::cell::*;
use neb::ram::schema::*;
use neb::ram::chunk::Chunks;
use neb::ram::types::*;
use neb::ram::io::writer;
use neb::ram::cleaner::Cleaner;
use neb::server::ServerMeta;
use env_logger;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std;
use std::rc::Rc;
use super::*;

pub const CHUNK_SIZE: usize = 1 * 8 * 1024 * 1024;

#[test]
pub fn round_robin_segment () {
    let num = AtomicU8::new(std::u8::MAX);
    assert_eq!(num.load(Ordering::SeqCst), 255);
    assert_eq!(num.fetch_add(1, Ordering::SeqCst), 255);
    assert_eq!(num.fetch_add(1, Ordering::SeqCst), 0);
    assert_eq!(num.fetch_add(1, Ordering::SeqCst), 1);
}

#[test]
pub fn cell_rw () {
    env_logger::init();
    info!("START");
    let id1 = Id::new(1, 1);
    let id2 = Id::new(1, 2);
    let fields = default_fields();
    let mut schema = Schema::new(String::from("dummy"), None, fields);
    let mut data_map = Map::<Value>::new();
    data_map.insert(&String::from("id"), Value::I64(100));
    data_map.insert(&String::from("score"), Value::U64(70));
    data_map.insert(&String::from("name"), Value::String(String::from("Jack")));
    let mut data = Value::Map(data_map);
    let schemas = SchemasServer::new(None).unwrap();
    let chunks = Chunks::new(1, CHUNK_SIZE, Arc::<ServerMeta>::new(ServerMeta {
        schemas: schemas.clone()
    }), None);
    schemas.new_schema(schema.clone());
    let mut cell = Cell {
        header: Header::new(0, schema.id, &id1),
        data: data
    };
    let header = chunks.write_cell(&mut cell).unwrap();
    let cell_1_ptr = chunks.chunk_ptr(&Id::from_header(&header));
    {
        let stored_cell = chunks.read_cell(&id1).unwrap();
        assert!(stored_cell.header.size > (4 + HEADER_SIZE) as u32);
        assert!(stored_cell.header.size > (4 + HEADER_SIZE) as u32);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 100);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "Jack");
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 70);
    }
    data_map = Map::<Value>::new();
    data_map.insert(&String::from("id"), Value::I64(2));
    data_map.insert(&String::from("score"), Value::U64(80));
    data_map.insert(&String::from("name"), Value::String(String::from("John")));
    data = Value::Map(data_map);
    cell = Cell {
        header: Header::new(0, schema.id, &id2),
        data: data
    };
    let header = chunks.write_cell(&mut cell).unwrap();
    let cell_2_ptr = chunks.chunk_ptr(&Id::from_header(&header));
    assert_eq!(cell_2_ptr, cell_1_ptr + cell.header.size as usize);
    {
        let stored_cell = chunks.read_cell(&id2).unwrap();
        assert!(stored_cell.header.size > (4 + HEADER_SIZE) as u32);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 2);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 80);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "John");
    }
    {
        let stored_cell = chunks.read_cell(&id1).unwrap();
        assert!(stored_cell.header.size > (4 + HEADER_SIZE) as u32);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 100);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "Jack");
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 70);
    }
    data_map = Map::<Value>::new();
    data_map.insert(&String::from("id"), Value::I64(2));
    data_map.insert(&String::from("score"), Value::U64(95));
    data_map.insert(&String::from("name"), Value::String(String::from("John")));
    data = Value::Map(data_map);
    cell = Cell {
        header: Header::new(0, schema.id, &id2),
        data: data
    };
    let header = chunks.update_cell(&mut cell).unwrap();
    let cell_2_ptr = chunks.chunk_ptr(&Id::from_header(&header));
    {
        let stored_cell = chunks.read_cell(&id2).unwrap();
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 2);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 95);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "John");
    }
    {
        let stored_cell = chunks.read_cell(&id1).unwrap();
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 100);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "Jack");
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 70);
    }
    chunks.update_cell_by(&id2, |mut cell| {
        let mut data_map = Map::<Value>::new();
        data_map.insert(&String::from("id"), Value::I64(2));
        data_map.insert(&String::from("score"), Value::U64(100));
        data_map.insert(&String::from("name"), Value::String(String::from("John")));
        let data = Value::Map(data_map);
        cell.data = data;
        Some(cell)
    }).unwrap();
    {
        let stored_cell = chunks.read_cell(&id2).unwrap();
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 2);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 100);
        assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "John");
    }
    chunks.remove_cell(&id1).unwrap();
    assert!(chunks.read_cell(&id1).is_err());
    /////////////////////////// TESET CLEANER ///////////////////////////
    debug!("Testing cleaner");
    Cleaner::clean_chunks(&chunks);
    let stored_cell = chunks.read_cell(&id2).unwrap();
    assert_eq!(stored_cell.data.Map().unwrap().get_static_key("id").unwrap().I64().unwrap(), 2);
    assert_eq!(stored_cell.data.Map().unwrap().get_static_key("score").unwrap().U64().unwrap(), 100);
    assert_eq!(stored_cell.data.Map().unwrap().get_static_key("name").unwrap().String().unwrap(), "John");

    let cell_size = stored_cell.header.size;
    let seg = &chunks.list[0].segs[0];
    assert_eq!(seg.append_header.load(Ordering::SeqCst), seg.addr + cell_size as usize);
}