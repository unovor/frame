// Copyright 2015-2020 Parity Technologies (UK) Ltd.
// This file is part of Parity.

// Parity is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity.  If not, see <http://www.gnu.org/licenses/>.

/// Database statistics.

use std::sync::atomic::{AtomicU64, AtomicU32, Ordering};
use std::mem::MaybeUninit;
use std::io::{Read, Write, Cursor};
use crate::{error::Result, column::ColId};

const HISTOGRAM_BUCKETS: usize = 2048;
const HISTOGRAM_BUCKET_BITS: u8 = 4;
const SIZE_TIERS: usize = 16;

pub const TOTAL_SIZE: usize = 4 * HISTOGRAM_BUCKETS + 8 * SIZE_TIERS + 8 * 10;

pub struct ColumnStats {
	value_histogram: [AtomicU32; HISTOGRAM_BUCKETS],
	query_histogram: [AtomicU64; SIZE_TIERS], // Per size tier
	oversized: AtomicU64,
	oversized_bytes: AtomicU64,
	total_values: AtomicU64,
	total_bytes: AtomicU64,
	commits: AtomicU64,
	inserted_new: AtomicU64,
	inserted_overwrite: AtomicU64,
	removed_hit: AtomicU64,
	removed_miss: AtomicU64,
	queries_miss: AtomicU64,
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> AtomicU32 {
	let mut buf = [0u8; 4];
	cursor.read_exact(&mut buf).expect("Incorrect stats buffer");
	AtomicU32::new(u32::from_le_bytes(buf))
}

fn read_u64(cursor: &mut Cursor<&[u8]>) -> AtomicU64 {
	let mut buf = [0u8; 8];
	cursor.read_exact(&mut buf).expect("Incorrect stats buffer");
	AtomicU64::new(u64::from_le_bytes(buf))
}

fn write_u32(cursor: &mut Cursor<&mut [u8]>, val: &AtomicU32) {
	cursor.write(&val.load(Ordering::Relaxed).to_le_bytes()).expect("Incorrent stats buffer");
}

fn write_u64(cursor: &mut Cursor<&mut [u8]>, val: &AtomicU64) {
	cursor.write(&val.load(Ordering::Relaxed).to_le_bytes()).expect("Incorrent stats buffer");
}

fn value_histogram_index(size: u32) -> Option<usize> {
	let bucket = size as usize >> HISTOGRAM_BUCKET_BITS;
	if bucket < HISTOGRAM_BUCKETS {
		Some(bucket)
	} else {
		None
	}
}

impl ColumnStats {
	pub fn from_slice(data: &[u8]) -> ColumnStats {
		let mut cursor = Cursor::new(data);
		let mut value_histogram: [AtomicU32; HISTOGRAM_BUCKETS] = unsafe { MaybeUninit::uninit().assume_init() };
		for n in 0 .. HISTOGRAM_BUCKETS {
			value_histogram[n] = read_u32(&mut cursor);
		}
		let mut query_histogram: [AtomicU64; SIZE_TIERS] = unsafe { MaybeUninit::uninit().assume_init() };
		for n in 0 .. SIZE_TIERS {
			query_histogram[n] = read_u64(&mut cursor);
		}
		ColumnStats {
			value_histogram,
			query_histogram,
			oversized: read_u64(&mut cursor),
			oversized_bytes: read_u64(&mut cursor),
			total_values: read_u64(&mut cursor),
			total_bytes: read_u64(&mut cursor),
			commits: read_u64(&mut cursor),
			inserted_new: read_u64(&mut cursor),
			inserted_overwrite: read_u64(&mut cursor),
			removed_hit: read_u64(&mut cursor),
			removed_miss: read_u64(&mut cursor),
			queries_miss: read_u64(&mut cursor),
		}
	}

	pub fn empty() -> ColumnStats {
		let value_histogram: [AtomicU32; HISTOGRAM_BUCKETS] = unsafe { std::mem::transmute([0u32; HISTOGRAM_BUCKETS]) };
		let query_histogram: [AtomicU64; SIZE_TIERS] = unsafe { std::mem::transmute([0u64; SIZE_TIERS]) };
		ColumnStats {
			value_histogram,
			query_histogram,
			oversized: Default::default(),
			oversized_bytes: Default::default(),
			total_values: Default::default(),
			total_bytes: Default::default(),
			commits: Default::default(),
			inserted_new: Default::default(),
			inserted_overwrite: Default::default(),
			removed_hit: Default::default(),
			removed_miss: Default::default(),
			queries_miss: Default::default(),
		}
	}

	pub fn to_slice(&self, data: &mut [u8]) {
		let mut cursor = Cursor::new(data);
		for n in 0 .. HISTOGRAM_BUCKETS {
			write_u32(&mut cursor, &self.value_histogram[n]);
		}
		for n in 0 .. SIZE_TIERS {
			write_u64(&mut cursor, &self.query_histogram[n]);
		}
		write_u64(&mut cursor, &self.oversized);
		write_u64(&mut cursor, &self.oversized_bytes);
		write_u64(&mut cursor, &self.total_values);
		write_u64(&mut cursor, &self.total_bytes);
		write_u64(&mut cursor, &self.commits);
		write_u64(&mut cursor, &self.inserted_new);
		write_u64(&mut cursor, &self.inserted_overwrite);
		write_u64(&mut cursor, &self.removed_hit);
		write_u64(&mut cursor, &self.removed_miss);
		write_u64(&mut cursor, &self.queries_miss);
	}

	fn write_file(&self, file: &std::fs::File, col: ColId) -> Result<()> {
		let mut writer = std::io::BufWriter::new(file);
		writeln!(writer, "Column {}", col)?;
		writeln!(writer, "Total values: {}", self.total_values.load(Ordering::Relaxed))?;
		writeln!(writer, "Total bytes: {}", self.total_bytes.load(Ordering::Relaxed))?;
		writeln!(writer, "Total oversized values: {}", self.oversized.load(Ordering::Relaxed))?;
		writeln!(writer, "Total oversized bytes: {}", self.oversized_bytes.load(Ordering::Relaxed))?;
		writeln!(writer, "Total commits: {}", self.commits.load(Ordering::Relaxed))?;
		writeln!(writer, "New value insertions: {}", self.inserted_new.load(Ordering::Relaxed))?;
		writeln!(writer, "Existing value insertions: {}", self.inserted_overwrite.load(Ordering::Relaxed))?;
		writeln!(writer, "Removals: {}", self.removed_hit.load(Ordering::Relaxed))?;
		writeln!(writer, "Missed removals: {}", self.removed_miss.load(Ordering::Relaxed))?;
		write!(writer, "Queries per size tier: [")?;
		for i in 0 .. SIZE_TIERS {
			if i == SIZE_TIERS - 1 {
				write!(writer, "{}]\n", self.query_histogram[i].load(Ordering::Relaxed))?;
			} else {
				write!(writer, "{}, ", self.query_histogram[i].load(Ordering::Relaxed))?;
			}
		}
		writeln!(writer, "Missed queries: {}", self.queries_miss.load(Ordering::Relaxed))?;
		writeln!(writer, "Value histogram:")?;
		for i in 0 .. HISTOGRAM_BUCKETS {
			let count = self.value_histogram[i].load(Ordering::Relaxed);
			if count != 0 {
				writeln!(writer,
					"    {}-{}: {}",
					i << HISTOGRAM_BUCKET_BITS,
					(((i + 1) << HISTOGRAM_BUCKET_BITS) - 1)
					, count
				)?;
			}
		}
		writeln!(writer, "")?;
		Ok(())
	}

	pub fn write_summary(&self, file: &std::fs::File, col: ColId) {
		let _ = self.write_file(file, col);
	}

	pub fn query_hit(&self, size_tier: u8) {
		self.query_histogram[size_tier as usize].fetch_add(1, Ordering::Relaxed);
	}

	pub fn query_miss(&self) {
		self.queries_miss.fetch_add(1, Ordering::Relaxed);
	}

	pub fn insert(&self, size: u32) {
		if let Some(index) = value_histogram_index(size) {
			self.value_histogram[index].fetch_add(1, Ordering::Relaxed);
		} else {
			self.oversized.fetch_add(1, Ordering::Relaxed);
			self.oversized_bytes.fetch_add(size as u64, Ordering::Relaxed);
		}
		self.total_values.fetch_add(1, Ordering::Relaxed);
		self.total_bytes.fetch_add(size as u64, Ordering::Relaxed);
	}

	pub fn remove(&self, size: u32) {
		if let Some(index) = value_histogram_index(size) {
			self.value_histogram[index].fetch_sub(1, Ordering::Relaxed);
		} else {
			self.oversized.fetch_sub(1, Ordering::Relaxed);
			self.oversized_bytes.fetch_sub(size as u64, Ordering::Relaxed);
		}
		self.total_values.fetch_sub(1, Ordering::Relaxed);
		self.total_bytes.fetch_sub(size as u64, Ordering::Relaxed);
	}

	pub fn insert_val(&self, size: u32) {
		self.inserted_new.fetch_add(1, Ordering::Relaxed);
		self.insert(size);
	}

	pub fn remove_val(&self, size: u32) {
		self.removed_hit.fetch_add(1, Ordering::Relaxed);
		self.remove(size);
	}

	pub fn remove_miss(&self) {
		self.removed_miss.fetch_add(1, Ordering::Relaxed);
	}

	pub fn replace_val(&self, old: u32, new: u32) {
		self.inserted_overwrite.fetch_add(1, Ordering::Relaxed);
		self.remove(old);
		self.insert(new);
	}

	pub fn commit(&self) {
		self.commits.fetch_add(1, Ordering::Relaxed);
	}
}



