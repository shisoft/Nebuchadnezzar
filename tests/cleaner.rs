use neb::ram::cell;
use neb::ram::cell::*;
use neb::ram::schema::*;
use neb::ram::chunk::Chunks;
use neb::ram::types;
use neb::ram::types::*;
use neb::ram::io::writer;
use neb::ram::cleaner::Cleaner;
use neb::ram::schema::Field;
use neb::server::ServerMeta;
use neb::ram::cleaner::*;
use neb::ram::tombstone::Tombstone;
use neb::ram::entry::{EntryType, EntryContent};
use env_logger;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std;
use std::rc::Rc;
use super::*;
use neb::ram::chunk::Chunk;

pub const DATA_SIZE: usize = 1000 * 1024; // nearly 1MB
const MAX_SEGMENT_SIZE: usize = 8 * 1024 * 1024;

fn default_cell(id: &Id) -> Cell {
    let data: Vec<_> =
        std::iter::repeat(Value::U8(id.lower as u8))
            .take(DATA_SIZE)
            .collect();
    Cell {
        header: CellHeader::new(0, 0, id, 0),
        data: data_map_value!(id: id.lower as i32, data: data)
    }
}

fn default_fields () -> Field {
    Field::new ("*", 0, false, false, Some(
        vec![
            Field::new("id", type_id_of(Type::I32), false, false, None),
            Field::new("data", type_id_of(Type::U8), false, true, None)
        ]
    ))
}

fn seg_positions(chunk: &Chunk) -> Vec<(u64, usize)> {
    chunk.addrs_seg
        .read()
        .iter()
        .map(|(pos, hash)| (*hash, *pos))
        .collect()
}

#[test]
pub fn full_clean_cycle() {
    env_logger::init();
    let schema = Schema::new(
        "cleaner_test",
        None,
        default_fields(),
        false);
    let schemas = LocalSchemasCache::new("", None).unwrap();
    schemas.new_schema(schema);
    let chunks = Chunks::new(
        1, // single chunk
        MAX_SEGMENT_SIZE * 2, // chunk two segments
        Arc::new(ServerMeta { schemas }),
        None);
    let chunk = &chunks.list[0];

    assert_eq!(chunk.segments().len(), 1);

    // put 16 cells to fill up all of those segments allocated
    for i in 0..16 {
        let mut cell = default_cell(&Id::new(0, i));
        chunks.write_cell(&mut cell).unwrap();

    }

    assert_eq!(chunk.segments().len(), 2);
    assert_eq!(chunk.index.len(), 16);

    println!("trying to delete cells");

    let all_seg_positions = seg_positions(chunk);
    let all_cell_addresses = chunk.cell_addresses();
    assert_eq!(all_seg_positions.len(), 2);
    assert_eq!(all_cell_addresses.len(), 16);

    for i in 0..8 {
        chunks.remove_cell(&Id::new(0, i * 2));
    }
    // try to scan first segment expect no panic
    println!("Scanning first segment...");
    chunk.live_entries(&chunk.segments()[0]);

    println!("Scanning second segment for tombstones...");
    let live_entries = chunk.live_entries(&chunk.segments()[1]);
    let tombstones: Vec<_> = live_entries
        .iter()
        .filter(|e| e.meta.entry_header.entry_type == EntryType::Tombstone)
        .collect();
    for i in 0..tombstones.len() {
        let hash = (i * 2) as u64;
        let e = &tombstones[i];
        assert_eq!(e.meta.entry_header.entry_type, EntryType::Tombstone);
        if let EntryContent::Tombstone(ref t) = e.content {
            assert_eq!(t.hash, hash);
            assert_eq!(t.partition, 0);
        } else { panic!(); }
    }

    assert_eq!(chunk.cell_addresses().len(), 8);

    chunk.apply_dead_entry();

    // Compact all segments
    chunk.segments().into_iter()
        .for_each(|seg|
            compact::CompactCleaner::clean_segment(chunk, &seg));

    let compacted_seg_positions = seg_positions(chunk);
    let compacted_cell_addresses = chunk.cell_addresses();
    assert_eq!(compacted_seg_positions.len(), 2);
    assert_eq!(compacted_cell_addresses.len(), 8);

}