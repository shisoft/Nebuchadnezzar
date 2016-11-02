use ram::schema::Schema;
use std::mem;
use serde_json;
use ram::io::reader;
use ram::types::{Map, Value};

const MAX_CELL_SIZE :i32 = 1 * 1024 * 1024;

pub type DataValue = Value;
pub type DataMap = Map<String, Value>;

#[repr(packed)]
#[derive(Debug, Copy, Clone)]
pub struct Header {
    pub version: u64,
    pub size: u32,
    pub schema: u32,
    pub hash: u64,
    pub partation: u64
}

pub const HEADER_SIZE :usize = 32;

pub struct Cell {
    pub header: Header,
    pub data: DataValue
}

impl Cell {

    pub fn from_raw(ptr: usize, schema: Schema) -> Cell {
        let header = unsafe {(*(ptr as *const Header))};
        let data_ptr = ptr + HEADER_SIZE;
        Cell {
            header: header,
            data: reader::read_by_schema(data_ptr, schema)
        }
    }

}