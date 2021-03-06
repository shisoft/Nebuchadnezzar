use itertools::Itertools;
use lightning::map::{Map, ObjectMap};
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use dovahkiin::types::SharedValue;

use crate::ram::{
    cell::{header_from_chunk_raw, select_from_chunk_raw},
    chunk::Chunk,
    clock::now,
};

mod histogram;
pub mod sm;

pub struct SchemaStatistics {
    pub histogram: HashMap<u64, [HistogramKey; HISTOGRAM_TARGET_BUCKETS + 1]>,
    pub count: usize,
    pub segs: usize,
    pub bytes: usize,
    pub timestamp: u32,
}

pub struct ChunkStatistics {
    pub schemas: ObjectMap<Arc<SchemaStatistics>>,
}

const HISTOGRAM_PARTITATION_SIZE: usize = 1024;
const HISTOGRAM_PARTITATION_BUCKETS: usize = 128;
const HISTOGRAM_TARGET_BUCKETS: usize = 100;

type HistogramKey = [u8; 8];

impl ChunkStatistics {
    pub fn from_chunk(chunk: &Chunk) -> Self {
        let histogram_partitations = chunk
            .cell_index
            .entries()
            .chunks(HISTOGRAM_PARTITATION_SIZE)
            .map(|s| s.to_vec())
            .collect_vec();
        let partitations: Vec<_> = histogram_partitations
            .into_par_iter()
            .map(|partitation| build_partitation_statistics(partitation, chunk))
            .collect();
        let schema_ids: Vec<_> = partitations
            .iter()
            .map(|(sizes, _, _, _)| sizes.keys())
            .flatten()
            .dedup()
            .collect();
        let total_size = schema_ids
            .iter()
            .map(|sid| {
                (
                    *sid,
                    partitations
                        .iter()
                        .map(|(sizes, _, _, _)| sizes.get(sid).unwrap_or(&0))
                        .sum::<usize>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let total_counts = schema_ids
            .iter()
            .map(|sid| {
                (
                    *sid,
                    partitations
                        .iter()
                        .map(|(_, _, counts, _)| counts.get(sid).unwrap_or(&0))
                        .sum::<usize>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let total_segs = schema_ids
            .iter()
            .map(|sid| {
                (
                    *sid,
                    partitations
                        .iter()
                        .map(|(_, segs, _, _)| segs.get(sid).map(|set| set.len()).unwrap_or(0))
                        .sum::<usize>(),
                )
            })
            .collect::<HashMap<_, _>>();
        let empty_histo = Default::default();
        let mut schema_histograms = schema_ids
            .iter()
            .map(|sid| {
                (*sid, {
                    let parted_histos = partitations
                        .iter()
                        .map(|(_, _, _, histo)| histo.get(sid).unwrap_or(&empty_histo))
                        .collect_vec();
                    let field_ids = parted_histos
                        .iter()
                        .map(|histo_map| histo_map.keys())
                        .flatten()
                        .dedup()
                        .collect::<Vec<_>>();
                    field_ids
                        .par_iter()
                        .map(|field_id| {
                            let schema_field_histograms = parted_histos
                                .iter()
                                .map(|histo_map| &histo_map[field_id])
                                .collect_vec();
                            (**field_id, build_histogram(schema_field_histograms))
                        })
                        .collect::<HashMap<u64, _>>()
                })
            })
            .collect::<HashMap<_, _>>();
        let schema_statistics = ObjectMap::<Arc<SchemaStatistics>>::with_capacity(
            schema_ids.capacity().next_power_of_two(),
        );
        for schema_id in schema_ids {
            let statistics = SchemaStatistics {
                histogram: schema_histograms.remove(&schema_id).unwrap(),
                count: *total_counts.get(&schema_id).unwrap(),
                segs: *total_segs.get(&schema_id).unwrap(),
                bytes: *total_size.get(&schema_id).unwrap(),
                timestamp: now(),
            };
            schema_statistics.insert(&(*schema_id as usize), Arc::new(statistics));
        }
        Self {
            schemas: schema_statistics,
        }
    }
}

fn build_partitation_statistics(
    partitation: Vec<(usize, usize)>,
    chunk: &Chunk,
) -> (
    HashMap<u32, usize>,
    HashMap<u32, HashSet<usize>>,
    HashMap<u32, usize>,
    HashMap<u32, HashMap<u64, (Vec<HistogramKey>, usize, usize)>>,
) {
    // Build exact histogram for each of the partitation and then approximate overall histogram
    let mut sizes = HashMap::new();
    let mut segs = HashMap::new();
    let mut counts = HashMap::new();
    let mut exact_accumlators = HashMap::new();
    let partitation_size = partitation.len();
    for (hash, _) in partitation {
        let loc = if let Ok(ptr) = chunk.location_for_read(hash as u64) {
            ptr
        } else {
            trace!("Cannot obtain cell lock {} for statistics", hash);
            continue;
        };
        match header_from_chunk_raw(*loc) {
            Ok((header, _, entry_header)) => {
                let cell_size = entry_header.content_length as usize;
                let cell_seg = chunk.allocator.id_by_addr(*loc);
                let schema_id = header.schema;
                if let Some(schema) = chunk.meta.schemas.get(&schema_id) {
                    let fields = schema.index_fields.keys().cloned().collect_vec();
                    if let Ok(partial_cell) = select_from_chunk_raw(*loc, chunk, fields.as_slice())
                    {
                        let field_array = if fields.len() == 1 {
                            vec![partial_cell]
                        } else if let SharedValue::Array(arr) = partial_cell {
                            arr
                        } else {
                            error!(
                                "Cannot decode partial cell for statistics {:?}",
                                partial_cell
                            );
                            continue;
                        };
                        for (i, val) in field_array.into_iter().enumerate() {
                            if val == SharedValue::Null || val == SharedValue::NA {
                                continue;
                            }
                            let field_id = fields[i];
                            exact_accumlators
                                .entry(schema_id)
                                .or_insert_with(|| HashMap::new())
                                .entry(field_id)
                                .or_insert_with(|| Vec::with_capacity(partitation_size))
                                .push(val.feature());
                        }
                        *counts.entry(schema_id).or_insert(0) += 1;
                        *sizes.entry(schema_id).or_insert(0) += cell_size;
                        segs.entry(schema_id)
                            .or_insert_with(|| HashSet::new())
                            .insert(cell_seg);
                    }
                } else {
                    warn!("Cannot get schema {} for statistics", schema_id);
                }
            }
            Err(e) => {
                warn!("Failed to read {} for statistics, error: {:?}", hash, e);
            }
        }
    }
    let histograms: HashMap<_, _> = exact_accumlators
        .into_iter()
        .map(|(schema_id, schema_histograms)| {
            let compiled_histograms = schema_histograms
                .into_iter()
                .map(|(field, items)| {
                    let num_items = items.len();
                    let (histogram, depth) = build_partitation_histogram(items);
                    (field, (histogram, num_items, depth))
                })
                .collect::<HashMap<_, _>>();
            (schema_id, compiled_histograms)
        })
        .collect::<HashMap<_, _>>();
    (sizes, segs, counts, histograms)
}

fn build_partitation_histogram(mut items: Vec<HistogramKey>) -> (Vec<HistogramKey>, usize) {
    if items.len() <= HISTOGRAM_PARTITATION_BUCKETS {
        return (items, 1);
    }
    items.sort();
    let depth = items.len() / HISTOGRAM_PARTITATION_BUCKETS;
    let mut histogram = (0..HISTOGRAM_PARTITATION_BUCKETS)
        .map(|tile| items[tile * depth])
        .collect_vec();
    let last_item = &items[items.len() - 1];
    if histogram.last().unwrap() != last_item {
        histogram.push(*last_item);
    }
    (histogram, depth)
}

fn build_histogram(
    partitations: Vec<&(Vec<HistogramKey>, usize, usize)>,
) -> [HistogramKey; HISTOGRAM_TARGET_BUCKETS + 1] {
    // Build the approximated histogram from partitation histograms
    // https://arxiv.org/abs/1606.05633
    let mut part_idxs = vec![0; partitations.len()];
    let part_histos = partitations
        .iter()
        .map(|(histo, _, _)| histo)
        .filter(|histo| !histo.is_empty())
        .collect_vec();
    let num_total = partitations.iter().map(|(_, num, _)| num).sum::<usize>();
    let part_depths = partitations
        .iter()
        .map(|(_, _, depth)| *depth)
        .collect_vec();
    let target_width = num_total / HISTOGRAM_TARGET_BUCKETS;
    let mut target_histogram = [[0u8; 8]; HISTOGRAM_TARGET_BUCKETS + 1];
    // Perform a merge sort for sorted pre-histogram
    let mut filled = target_width;
    let mut last_key = Default::default();
    'HISTO_CONST: for i in 0..HISTOGRAM_PARTITATION_BUCKETS {
        loop {
            let (key, ended) = if let Some((part_idx, histo)) = part_histos
                .iter()
                .enumerate()
                .filter(|(i, h)| {
                    let idx = part_idxs[*i];
                    idx < h.len()
                })
                .min_by(|(i1, h1), (i2, h2)| {
                    let h1_idx = part_idxs[*i1];
                    let h2_idx = part_idxs[*i2];
                    h1[h1_idx].cmp(&h2[h2_idx])
                }) {
                let histo_idx = part_idxs[part_idx];
                part_idxs[part_idx] += 1;
                ((histo[histo_idx], part_idx), false)
            } else {
                (last_key, true)
            };
            last_key = key;
            let idx = last_key.1;
            if filled >= target_width || ended {
                target_histogram[i] = last_key.0;
                filled = 0;
                continue 'HISTO_CONST;
            }
            filled += part_depths[idx];
        }
    }
    target_histogram
}

#[cfg(test)]
mod tests {
    use dovahkiin::types::OwnedValue;

    use super::*;

    #[test]
    fn partitation_histogram() {
        let small_set = (0..10).map(|n| OwnedValue::U64(n).feature()).collect_vec();
        assert_eq!(
            build_partitation_histogram(small_set.clone()),
            (small_set, 1)
        );
        let eq_set = (0..HISTOGRAM_PARTITATION_BUCKETS)
            .map(|n| OwnedValue::U64(n as u64).feature())
            .collect_vec();
        assert_eq!(
            build_partitation_histogram(eq_set.clone()),
            (eq_set, 1)
        );

        let double_set = (0..HISTOGRAM_PARTITATION_BUCKETS * 2)
        .map(|n| OwnedValue::U64(n as u64).feature())
        .collect_vec();
        let mut expect = double_set.iter().step_by(2).cloned().collect_vec();
        expect.push(double_set.last().unwrap().to_owned());
        assert_eq!(
            build_partitation_histogram(double_set),
            (expect, 2)
        );

        let triple_set = (0..HISTOGRAM_PARTITATION_BUCKETS * 3)
        .map(|n| OwnedValue::U64(n as u64).feature())
        .collect_vec();
        let mut expect = triple_set.iter().step_by(3).cloned().collect_vec();
        expect.push(triple_set.last().unwrap().to_owned());
        assert_eq!(
            build_partitation_histogram(triple_set),
            (expect, 3)
        );
    }
}
