use crate::ram::types;

pub const CHUNK_SIZE: usize = 2048;

macro_rules! test_nums {
    (
        $t:ident, $io:ident
    ) => {
        mod $t {
            use super::CHUNK_SIZE;
            use crate::ram::chunk::Chunks;
            use crate::ram::types;
            use rand;
            use std;
            #[test]
            fn test() {
                let rand_num = rand::random::<$t>();
                let test_data = vec![
                    std::$t::MIN,
                    std::$t::MAX,
                    rand_num,
                    0 as $t,
                    1 as $t,
                    2 as $t,
                    127 as $t,
                ];
                let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
                let counts = CHUNK_SIZE / types::$io::size(0) as usize;
                let chunk_addr = chunk.segments()[0].addr;
                for d in test_data {
                    for i in 0..counts {
                        let addr = chunk_addr + i * types::$io::size(0);
                        types::$io::write(&d, addr);
                        assert!(types::$io::read(addr) == d);
                    }
                    for i in 0..counts {
                        let addr = chunk_addr + i * types::$io::size(0);
                        assert!(types::$io::read(addr) == d);
                    }
                }
            }
        }
    };
}

test_nums!(i8, i8_io);
test_nums!(i16, i16_io);
test_nums!(i32, i32_io);
test_nums!(i64, i64_io);

test_nums!(u8, u8_io);
test_nums!(u16, u16_io);
test_nums!(u32, u32_io);
test_nums!(u64, u64_io);

test_nums!(f32, f32_io);
test_nums!(f64, f64_io);

mod pos2d32 {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use rand;
    use std;
    #[test]
    fn test() {
        let test_data = vec![
            types::Pos2d32 {
                x: std::f32::MIN,
                y: std::f32::MAX,
            },
            types::Pos2d32 {
                x: rand::random::<f32>(),
                y: rand::random::<f32>(),
            },
        ];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let counts = CHUNK_SIZE / types::pos2d32_io::size(0) as usize;
        let chunk_addr = chunk.segments()[0].addr;
        for d in test_data {
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos2d32_io::size(0);
                types::pos2d32_io::write(&d, addr);
                assert_eq!(types::pos2d32_io::read(addr), d);
            }
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos2d32_io::size(0);
                assert_eq!(types::pos2d32_io::read(addr), d);
            }
        }
    }
}

mod pos2d64 {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use rand;
    use std;
    #[test]
    fn test() {
        let test_data = vec![
            types::Pos2d64 {
                x: std::f64::MIN,
                y: std::f64::MAX,
            },
            types::Pos2d64 {
                x: rand::random::<f64>(),
                y: rand::random::<f64>(),
            },
        ];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let counts = CHUNK_SIZE / types::pos2d64_io::size(0) as usize;
        let chunk_addr = chunk.segments()[0].addr;
        for d in test_data {
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos2d64_io::size(0);
                types::pos2d64_io::write(&d, addr);
                assert_eq!(types::pos2d64_io::read(addr), d);
            }
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos2d64_io::size(0);
                assert_eq!(types::pos2d64_io::read(addr), d);
            }
        }
    }
}

mod pos3d32 {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use rand;
    use std;
    #[test]
    fn test() {
        let test_data = vec![
            types::Pos3d32 {
                x: std::f32::MIN,
                y: std::f32::MAX,
                z: rand::random::<f32>(),
            },
            types::Pos3d32 {
                x: rand::random::<f32>(),
                y: rand::random::<f32>(),
                z: rand::random::<f32>(),
            },
        ];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let counts = CHUNK_SIZE / types::pos3d32_io::size(0) as usize;
        let chunk_addr = chunk.segments()[0].addr;
        for d in test_data {
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos3d32_io::size(0);
                types::pos3d32_io::write(&d, addr);
                assert_eq!(types::pos3d32_io::read(addr), d);
            }
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos3d32_io::size(0);
                assert_eq!(types::pos3d32_io::read(addr), d);
            }
        }
    }
}

mod pos3d64 {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use rand;
    use std;
    #[test]
    fn test() {
        let test_data = vec![
            types::Pos3d64 {
                x: std::f64::MIN,
                y: std::f64::MAX,
                z: rand::random::<f64>(),
            },
            types::Pos3d64 {
                x: rand::random::<f64>(),
                y: rand::random::<f64>(),
                z: rand::random::<f64>(),
            },
        ];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let counts = CHUNK_SIZE / types::pos3d64_io::size(0) as usize;
        let chunk_addr = chunk.segments()[0].addr;
        for d in test_data {
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos3d64_io::size(0);
                types::pos3d64_io::write(&d, addr);
                assert_eq!(types::pos3d64_io::read(addr), d);
            }
            for i in 0..counts {
                let addr = chunk_addr + i * types::pos3d64_io::size(0);
                assert_eq!(types::pos3d64_io::read(addr), d);
            }
        }
    }
}

mod uuid {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use crate::ram::types::Id;
    #[test]
    fn test() {
        let test_data = vec![Id {
            higher: 1,
            lower: 2,
        }];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let counts = CHUNK_SIZE / types::id_io::size(0) as usize;
        let chunk_addr = chunk.segments()[0].addr;
        for d in test_data {
            for i in 0..counts {
                let addr = chunk_addr + i * types::id_io::size(0);
                types::id_io::write(&d, addr);
                assert_eq!(types::id_io::read(addr), d);
            }
            for i in 0..counts {
                let addr = chunk_addr + i * types::id_io::size(0);
                assert_eq!(types::id_io::read(addr), d);
            }
        }
    }
}

mod string {
    use super::CHUNK_SIZE;
    use crate::ram::chunk::Chunks;
    use crate::ram::types;
    use std::string::String;
    #[test]
    fn test() {
        let test_data = vec![
            "",
            "ಬಾ ಇಲ್ಲಿ ಸಂಭವಿಸ",
            "中文测试文本",
            "Hello Test",
            "🏳️‍🌈",
        ];
        let chunk = &Chunks::new_dummy(1, CHUNK_SIZE).list[0];
        let mut addr = chunk.segments()[0].addr;
        for d in test_data.clone() {
            let test_str = String::from(d);
            types::string_io::write(&test_str, addr);
            let len = types::string_io::size(addr);
            let dec_str = types::string_io::read(addr);
            assert_eq!(dec_str, test_str);
            println!("{}", dec_str);
            addr += len;
        }
        addr = chunk.segments()[0].addr;
        for d in test_data.clone() {
            let test_str = String::from(d);
            let len = types::string_io::size(addr);
            assert_eq!(types::string_io::read(addr), test_str);
            addr += len;
        }
    }
}

#[test]
fn array_len_type() {
    assert_eq!(
        types::u32_io::size(0),
        types::get_size(types::ARRAY_LEN_TYPE_ID, 0)
    )
}

#[test]
fn null_type() {
    assert_eq!(
        types::u8_io::size(0),
        types::get_size(types::NULL_TYPE_ID, 0)
    )
}

#[test]
fn _in_map() {
    let mut map2 = types::Map::new();
    map2.insert(&String::from("a"), types::Value::I32(1));
    map2.insert(&String::from("b"), types::Value::I64(2));

    let mut map = types::Map::new();
    map.insert(&String::from("A"), types::Value::I32(1));
    map.insert(&String::from("B"), types::Value::Map(map2));

    assert_eq!(map.get(&String::from("A")).I32().unwrap(), &1);
    assert_eq!(map.get_in(&["B", "a"]).I32().unwrap(), &1);

    map.set_in(&["B", "a"], types::Value::I32(20)).unwrap();
    assert_eq!(map.get_in(&["B", "a"]).I32().unwrap(), &20);

    map.update_in(&["B", "b"], |value: &mut types::Value| {
        assert_eq!(value.I64().unwrap(), &2);
        *value = types::Value::I64(30);
    });
    assert_eq!(map.get_in(&["B", "b"]).I64().unwrap(), &30);
}

#[test]
fn index_mut_map() {
    let mut value = types::Value::Map(types::Map::new());
    value["a"] = types::Value::String(String::from("A"));
    value["b"] = types::Value::Array(vec![types::Value::U64(5)]);
    assert_eq!(value["a"].String().unwrap(), &String::from("A"));
    assert_eq!(value["b"][0 as usize].U64().unwrap(), &5);
    value["b"][0 as usize] = types::Value::U64(10);
    assert_eq!(value["b"][0 as usize].U64().unwrap(), &10);
}

#[test]
fn num_cast() {
    assert_eq!(1u32, 1u64 as u32);
}