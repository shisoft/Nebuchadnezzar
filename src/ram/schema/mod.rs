use bifrost::raft::client::RaftClient;
use bifrost::raft::state_machine::master::ExecError;
use bifrost_hasher::hash_str;

use dovahkiin::types::Type;
use parking_lot::{RwLock, RwLockReadGuard};
use std::collections::HashMap;
use std::mem;

use super::types;
use core::borrow::Borrow;
use std::string::String;
use std::sync::Arc;

use futures::prelude::*;
use futures::FutureExt;
use std::ops::Deref;

pub mod sm;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Schema {
    pub id: u32,
    pub name: String,
    pub key_field: Option<Vec<u64>>,
    pub str_key_field: Option<Vec<String>>,
    pub id_index: HashMap<u64, Vec<usize>>,
    pub fields: Field,
    pub static_bound: usize,
    pub is_dynamic: bool,
    pub is_scannable: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum IndexType {
    Ranged,
    Hashed,
    Vectorized,
    Statistics,
}

impl Schema {
    pub fn new(
        name: &str,
        key_field: Option<Vec<String>>,
        mut fields: Field,
        is_dynamic: bool,
        is_scannable: bool,
    ) -> Schema {
        let mut bound = 0;
        let mut id_index = HashMap::new();
        fields.assign_offsets(&mut bound, &mut id_index, String::new(), vec![]);
        trace!("Schema {:?} has bound {}", fields, bound);
        Schema {
            id: 0,
            name: name.to_string(),
            key_field: match key_field {
                None => None,
                Some(ref keys) => Some(keys.iter().map(|f| hash_str(f)).collect()), // field list into field ids
            },
            str_key_field: key_field,
            static_bound: bound,
            fields,
            is_dynamic,
            is_scannable,
            id_index,
        }
    }
    pub fn new_with_id(
        id: u32,
        name: &str,
        key_field: Option<Vec<String>>,
        fields: Field,
        dynamic: bool,
        scannable: bool,
    ) -> Schema {
        let mut schema = Schema::new(name, key_field, fields, dynamic, scannable);
        schema.id = id;
        schema
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub data_type: Type,
    pub nullable: bool,
    pub is_array: bool,
    pub sub_fields: Option<Vec<Field>>,
    pub name: String,
    pub name_id: u64,
    pub indices: Vec<IndexType>,
    pub offset: Option<usize>,
}

impl Field {
    pub fn new(
        name: &str,
        data_type: Type,
        nullable: bool,
        is_array: bool,
        sub_fields: Option<Vec<Field>>,
        indices: Vec<IndexType>,
    ) -> Field {
        Field {
            name: name.to_string(),
            name_id: types::key_hash(name),
            data_type,
            nullable,
            is_array,
            sub_fields,
            indices,
            offset: None,
        }
    }
    fn assign_offsets(
        &mut self,
        offset: &mut usize,
        id_index: &mut HashMap<u64, Vec<usize>>,
        name_path: String,
        field_path: Vec<usize>,
    ) {
        const POINTER_SIZE: usize = mem::size_of::<u32>();
        self.offset = Some(*offset);
        let is_field_var = self.is_var();
        let name_path_hash = hash_str(&name_path);
        if self.nullable && !is_field_var {
            *offset += 1;
        }
        if self.is_array {
            // u32 as indication of the offset to the actual data
            *offset += POINTER_SIZE;
        } else if let Some(ref mut subs) = self.sub_fields {
            let format_name = if name_path.is_empty() {
                name_path
            } else {
                format!("{}|", name_path)
            };
            subs.iter_mut().enumerate().for_each(|(i, f)| {
                let mut new_path = field_path.clone();
                new_path.push(i);
                let new_name_path = format!("{}{}", format_name, f.name);
                f.assign_offsets(offset, id_index, new_name_path, new_path);
            });
        } else {
            if !is_field_var {
                *offset += types::size_of_type(self.data_type);
            } else {
                *offset += POINTER_SIZE;
            }
        }
        if !field_path.is_empty() {
            id_index.insert(name_path_hash, field_path);
        }
        trace!(
            "Assigned field {} to {:?}, now at {}, var {}, offset moved {}",
            self.name,
            self.offset,
            offset,
            is_field_var,
            *offset - self.offset.unwrap()
        );
    }
    pub fn is_var(&self) -> bool {
        self.is_array || !types::fixed_size(self.data_type)
    }
}

pub struct SchemasMap {
    schema_map: HashMap<u32, Schema>,
    name_map: HashMap<String, u32>,
    id_counter: u32,
}

pub struct LocalSchemasCache {
    map: Arc<RwLock<SchemasMap>>,
}

impl LocalSchemasCache {
    pub async fn new(
        group: &str,
        raft_client: &Arc<RaftClient>,
    ) -> Result<LocalSchemasCache, ExecError> {
        info!("Initializing local schema cache");
        let map = Arc::new(RwLock::new(SchemasMap::new()));
        let m1 = map.clone();
        let m2 = map.clone();
        let sm = sm::client::SMClient::new(sm::generate_sm_id(group), raft_client);
        let sm_data = sm.get_all().await?;
        {
            let mut map = map.write();
            debug!("Importing {} schemas from cluster", sm_data.len());
            for schema in sm_data {
                trace!("Importing schema {}", schema.name);
                map.new_schema(schema);
            }
        }
        debug!("Subscribing schema events...");
        let _ = sm
            .on_schema_added(move |schema| {
                debug!("Add schema {} from subscription", schema.id);
                let mut m1 = m1.write();
                m1.new_schema(schema);
                future::ready(()).boxed()
            })
            .await?;
        let _ = sm
            .on_schema_deleted(move |schema| {
                let mut m2 = m2.write();
                m2.del_schema(&schema).unwrap();
                future::ready(()).boxed()
            })
            .await?;
        let schemas = LocalSchemasCache { map };
        info!("Local schema initialization completed");
        return Ok(schemas);
    }
    pub fn new_local(_group: &str) -> Self {
        let map = Arc::new(RwLock::new(SchemasMap::new()));
        LocalSchemasCache { map }
    }
    pub fn get(&self, id: &u32) -> Option<ReadingSchema> {
        let m = self.map.read();
        let so = m.get(id).map(|s| s as *const Schema);
        so.map(|s| ReadingSchema {
            _owner: m,
            reference: s,
        })
    }
    pub fn new_schema(&self, schema: Schema) {
        // for debug only
        let mut m = self.map.write();
        m.new_schema(schema)
    }
    pub fn name_to_id(&self, name: &str) -> Option<u32> {
        let m = self.map.read();
        m.name_to_id(name)
    }
}

impl SchemasMap {
    pub fn new() -> SchemasMap {
        SchemasMap {
            schema_map: HashMap::new(),
            name_map: HashMap::new(),
            id_counter: 0,
        }
    }
    pub fn new_schema(&mut self, schema: Schema) {
        let name = schema.name.clone();
        let id = schema.id;
        self.schema_map.insert(id, schema);
        self.name_map.insert(name, id);
    }
    pub fn del_schema(&mut self, name: &str) -> Result<(), ()> {
        if let Some(id) = self.name_to_id(name) {
            self.schema_map.remove(&id);
        }
        self.name_map.remove(&name.to_string());
        Ok(())
    }
    pub fn get_by_name(&self, name: &str) -> Option<&Schema> {
        if let Some(id) = self.name_to_id(name) {
            return self.get(&id);
        }
        return None;
    }
    pub fn get(&self, id: &u32) -> Option<&Schema> {
        if let Some(schema) = self.schema_map.get(id) {
            return Some(schema);
        }
        return None;
    }
    pub fn name_to_id(&self, name: &str) -> Option<u32> {
        self.name_map.get(&name.to_string()).cloned()
    }
    fn next_id(&mut self) -> u32 {
        self.id_counter += 1;
        while self.schema_map.contains_key(&self.id_counter) {
            self.id_counter += 1;
        }
        self.id_counter
    }
    fn get_all(&self) -> Vec<Schema> {
        self.schema_map
            .values()
            .map(|s_ref| {
                let arc = s_ref.clone();
                let r: &Schema = arc.borrow();
                r.clone()
            })
            .collect()
    }
    fn load_from_list(&mut self, data: Vec<Schema>) {
        for schema in data {
            let id = schema.id;
            self.name_map.insert(schema.name.clone(), id);
            self.schema_map.insert(id, schema);
        }
    }
}

pub struct ReadingRef<O, T: ?Sized> {
    _owner: O,
    reference: *const T,
}

pub type ReadingSchema<'a> = ReadingRef<RwLockReadGuard<'a, SchemasMap>, Schema>;

impl<O, T: ?Sized> Deref for ReadingRef<O, T> {
    type Target = T;

    fn deref(&self) -> &T {
        unsafe { &*self.reference }
    }
}
