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

use std::collections::HashMap;
use std::io::{Read, Write, Seek};
use std::convert::TryInto;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use parking_lot::{Condvar, Mutex, RwLock, RwLockWriteGuard};
use crate::{
	error::{Error, Result},
	table::TableId as ValueTableId,
	index::{TableId as IndexTableId, Chunk as IndexChunk},
	options::Options,
};

pub struct InsertIndexAction {
	pub table: IndexTableId,
	pub index: u64,
}

pub struct InsertValueAction {
	pub table: ValueTableId,
	pub index: u64,
}

pub enum LogAction {
	BeginRecord(u64),
	InsertIndex(InsertIndexAction),
	InsertValue(InsertValueAction),
	DropTable(IndexTableId),
	EndRecord, // TODO: crc32
}

pub trait LogQuery {
	fn with_index<R, F: FnOnce(&IndexChunk) -> R> (&self, table: IndexTableId, index: u64, f: F) -> Option<R>;
	fn value(&self, table: ValueTableId, index: u64, dest: &mut[u8]) -> bool;
}

#[derive(Default)]
pub struct LogOverlays {
	index: HashMap<IndexTableId, IndexLogOverlay>,
	value: HashMap<ValueTableId, ValueLogOverlay>,
}

impl LogQuery for RwLock<LogOverlays> {
	fn with_index<R, F: FnOnce(&IndexChunk) -> R> (&self, table: IndexTableId, index: u64, f: F) -> Option<R> {
		self.read().index.get(&table).and_then(|o| o.map.get(&index).map(|(_id, data)| f(data)))
	}

	fn value(&self, table: ValueTableId, index: u64, dest: &mut[u8]) -> bool {
		let s = self.read();
		if let Some(d) = s.value.get(&table).and_then(|o| o.map.get(&index).map(|(_id, data)| data)) {
			let len = dest.len().min(d.len());
			dest[0..len].copy_from_slice(&d[0..len]);
			true
		} else {
			false
		}

	}
}

#[derive(Default)]
pub struct Cleared {
	index: Vec<(IndexTableId, u64)>,
	values: Vec<(ValueTableId, u64)>,
}

pub struct LogReader<'a> {
	file: RwLockWriteGuard<'a, std::io::BufReader<std::fs::File>>,
	record_id: u64,
	read_bytes: u64,
	cleared: Cleared,
}

impl<'a> LogReader<'a> {
	pub fn record_id(&self) -> u64 {
		self.record_id
	}

	fn new(
		file: RwLockWriteGuard<'a, std::io::BufReader<std::fs::File>>,
	) -> LogReader<'a> {
		LogReader {
			cleared: Default::default(),
			file,
			record_id: 0,
			read_bytes: 0,
		}
	}

	pub fn reset(&mut self) -> Result<()> {
		self.cleared = Default::default();
		self.file.seek(std::io::SeekFrom::Current(-(self.read_bytes as i64)))?;
		self.read_bytes = 0;
		self.record_id = 0;
		Ok(())
	}

	pub fn next(&mut self) -> Result<LogAction> {
		let mut buf = [0u8; 8];
		self.file.read_exact(&mut buf[0..1])?;
		self.read_bytes += 1;
		match buf[0] {
			1 =>  { // BeginRecord
				self.file.read_exact(&mut buf[0..8])?;
				let record_id = u64::from_le_bytes(buf);
				self.record_id = record_id;
				self.read_bytes += 8;
				//log::trace!(target: "parity-db", "Read record header {}", record_id);
				Ok(LogAction::BeginRecord(record_id))
			},
			2 => { // InsertIndex
				self.file.read_exact(&mut buf[0..2])?;
				let table = IndexTableId::from_u16(u16::from_le_bytes(buf[0..2].try_into().unwrap()));
				self.file.read_exact(&mut buf[0..8])?;
				let index = u64::from_le_bytes(buf);
				//log::trace!(target: "parity-db", "Read log index {}:{}", table, index);
				self.cleared.index.push((table, index));
				self.read_bytes += 10;
				Ok(LogAction::InsertIndex(InsertIndexAction { table, index }))
			},
			3 => { // InsertValue
				self.file.read_exact(&mut buf[0..2])?;
				let table = ValueTableId::from_u16(u16::from_le_bytes(buf[0..2].try_into().unwrap()));
				self.file.read_exact(&mut buf[0..8])?;
				let index = u64::from_le_bytes(buf);
				self.cleared.values.push((table, index));
				self.read_bytes += 10;
				//log::trace!(target: "parity-db", "Read log value {}:{}", table, index);
				Ok(LogAction::InsertValue(InsertValueAction { table, index }))
			},
			4 => {
				self.file.read_exact(&mut buf[0..4])?;
				self.read_bytes += 4;
				let checksum = u32::from_le_bytes(buf[0..4].try_into().unwrap());
				log::trace!(target: "parity-db", "Read end of record, checksum={:#x}", checksum);
				Ok(LogAction::EndRecord)
			},
			5 => {
				self.file.read_exact(&mut buf[0..2])?;
				let table = IndexTableId::from_u16(u16::from_le_bytes(buf[0..2].try_into().unwrap()));
				self.read_bytes += 2;
				//log::trace!(target: "parity-db", "Read drop table {}", table);
				Ok(LogAction::DropTable(table))
			}
			_ => Err(Error::Corruption("Bad log entry type".into()))
		}
	}

	pub fn read(&mut self, buf: &mut [u8]) -> Result<()> {
		self.read_bytes += buf.len() as u64;
		Ok(self.file.read_exact(buf)?)
	}

	pub fn drain(self) -> Cleared {
		self.cleared
	}

	pub fn read_bytes(&self) -> u64 {
		self.read_bytes
	}
}

pub struct LogChange {
	local_index: HashMap<IndexTableId, IndexLogOverlay>,
	local_values: HashMap<ValueTableId, ValueLogOverlay>,
	record_id: u64,
	dropped_tables: Vec<IndexTableId>,
}

impl LogChange {
	fn new(
		record_id: u64,
	) -> LogChange {
		LogChange {
			local_index: Default::default(),
			local_values: Default::default(),
			dropped_tables: Default::default(),
			record_id,
		}
	}

	pub fn to_file(self, file: &mut std::io::BufWriter<std::fs::File>)
		-> Result<(HashMap<IndexTableId, IndexLogOverlay>, HashMap<ValueTableId, ValueLogOverlay>, u64)>
	{
		let mut crc32 = crc32fast::Hasher::new();
		let mut bytes: u64 = 0;

		let mut write = |buf: &[u8]| -> Result<()> {
			file.write(buf)?;
			crc32.update(buf);
			bytes += buf.len() as u64;
			Ok(())
		};

		write(&1u8.to_le_bytes())?; // Begin record
		write(&self.record_id.to_le_bytes())?;

		for (id, overlay) in self.local_index.iter() {
			for (index, (_, chunk)) in overlay.map.iter() {
				write(&2u8.to_le_bytes().as_ref())?;
				write(&id.as_u16().to_le_bytes())?;
				write(&index.to_le_bytes())?;
				write(chunk)?;
			}
		}
		for (id, overlay) in self.local_values.iter() {
			for (index, (_, value)) in overlay.map.iter() {
				write(&3u8.to_le_bytes().as_ref())?;
				write(&id.as_u16().to_le_bytes())?;
				write(&index.to_le_bytes())?;
				write(value)?;
			}
		}
		for id in self.dropped_tables.iter() {
			log::debug!(target: "parity-db", "Finalizing drop {}", id);
			write(&5u8.to_le_bytes().as_ref())?;
			write(&id.as_u16().to_le_bytes())?;
		}

		write(&4u8.to_le_bytes())?; // End record
		let checksum: u32 = crc32.finalize();
		file.write(&checksum.to_le_bytes())?;
		bytes += 4;
		file.flush()?;
		Ok((self.local_index, self.local_values, bytes))
	}
}

pub struct LogWriter<'a> {
	overlays: &'a RwLock<LogOverlays>,
	log: LogChange,
}

impl<'a> LogWriter<'a> {
	fn new(
		overlays: &'a RwLock<LogOverlays>,
		record_id: u64,
	) -> LogWriter<'a> {
		LogWriter {
			overlays,
			log: LogChange::new(record_id),
		}
	}

	pub fn record_id(&self) -> u64 {
		self.log.record_id
	}

	pub fn insert_index(&mut self, table: IndexTableId, index: u64, data: &IndexChunk) {
		self.log.local_index.entry(table).or_default().map.insert(index, (self.log.record_id, data.clone()));
	}

	pub fn insert_value(&mut self, table: ValueTableId, index: u64, data: Vec<u8>) {
		self.log.local_values.entry(table).or_default().map.insert(index, (self.log.record_id, data.clone()));
	}

	pub fn drop_table(&mut self, id: IndexTableId) {
		self.log.dropped_tables.push(id);
	}

	pub fn drain(self) -> LogChange {
		self.log
	}
}

impl<'a> LogQuery for LogWriter<'a> {
	fn with_index<R, F: FnOnce(&IndexChunk) -> R> (&self, table: IndexTableId, index: u64, f: F) -> Option<R> {
		match self.log.local_index.get(&table).and_then(|o| o.map.get(&index).map(|(_id, data)| data)) {
			Some(data) => Some(f(data)),
			None => self.overlays.with_index(table, index, f),
		}
	}

	fn value(&self, table: ValueTableId, index: u64, dest: &mut[u8]) -> bool {
		if let Some(d) = self.log.local_values.get(&table).and_then(|o| o.map.get(&index).map(|(_id, data)| data)) {
			let len = dest.len().min(d.len());
			dest[0..len].copy_from_slice(&d[0..len]);
			true
		} else {
			self.overlays.value(table, index, dest)
		}

	}
}

#[derive(Default)]
pub struct IndexLogOverlay {
	pub map: HashMap<u64, (u64, IndexChunk)>, // index -> (record_id, entry)
}


#[derive(Default)]
pub struct ValueLogOverlay {
	pub map: HashMap<u64, (u64, Vec<u8>)>, // index -> (record_id, entry)
}

struct Appending {
	file: std::io::BufWriter<std::fs::File>,
	empty: bool,
}

struct Flushing {
	file: std::fs::File,
	empty: bool,
}

#[derive(Eq, PartialEq)]
enum ReadingState {
	Reading,
	Idle,
	Shutdown,
}

pub struct Log {
	overlays: RwLock<LogOverlays>,
	appending: RwLock<Appending>,
	reading: RwLock<std::io::BufReader<std::fs::File>>,
	reading_state: Mutex<ReadingState>,
	done_reading_cv: Condvar,
	flushing: Mutex<Flushing>,
	next_record_id: AtomicU64,
	dirty: AtomicBool,
	sync: bool,
}

impl Log {
	pub fn open(options: &Options) -> Result<Log> {
		let mut path = options.path.clone();
		path.push("log0");
		let (file0, id0) = Self::open_or_create_log_file(path.as_path())?;
		path.pop();
		path.push("log1");
		let (file1, id1) = Self::open_or_create_log_file(path.as_path())?;
		path.pop();
		path.push("log2");
		let (file2, id2) = Self::open_or_create_log_file(path.as_path())?;

		let mut ids = [(id0.unwrap_or(0), Some(file0)), (id1.unwrap_or(0), Some(file1)), (id2.unwrap_or(0), Some(file2))];
		ids.sort_by_key(|(id, _)|*id);
		log::debug!(target: "parity-db", "Opened logs ({} {} {})", ids[0].0, ids[1].0, ids[2].0);
		let reading = ids[0].1.take().unwrap();
		let flushing = ids[1].1.take().unwrap();
		let mut appending = ids[2].1.take().unwrap();
		appending.seek(std::io::SeekFrom::End(0))?;

		Ok(Log {
			overlays: Default::default(),
			appending: RwLock::new(Appending {
				file: std::io::BufWriter::new(appending),
				empty: false,
			}),
			reading: RwLock::new(std::io::BufReader::new(reading)),
			reading_state: Mutex::new(ReadingState::Reading),
			done_reading_cv: Condvar::new(),
			flushing: Mutex::new(Flushing {
				file: flushing,
				empty: false,
			}),
			next_record_id: AtomicU64::new(1),
			dirty: AtomicBool::new(true),
			sync: options.sync,
		})
	}

	pub fn open_or_create_log_file(path: &std::path::Path) -> Result<(std::fs::File, Option<u64>)> {
		let mut file = std::fs::OpenOptions::new().create(true).read(true).write(true).open(path)?;
		if file.metadata()?.len() == 0 {
			return Ok((file, None));
		}
		// read first record id
		let mut buf = [0; 9];
		file.read_exact(&mut buf)?;
		file.seek(std::io::SeekFrom::Start(0))?;
		let id = u64::from_le_bytes(buf[1..].try_into().unwrap());
		log::debug!(target: "parity-db", "Opened existing log {}, first record_id = {}", path.display(), id);

		Ok((file, Some(id)))
	}

	pub fn clear_logs(&self) -> Result<()> {
		let mut overlays = self.overlays.write();
		overlays.index.clear();
		overlays.value.clear();
		{
			let mut appending = self.appending.write();
			appending.empty = true;
			appending.file.flush()?;
			appending.file.get_mut().set_len(0)?;
		}
		{
			let mut flushing = self.flushing.lock();
			flushing.empty = true;
			flushing.file.set_len(0)?;
			flushing.file.seek(std::io::SeekFrom::Start(0))?;
		}
		{
			let mut reading = self.reading.write();
			reading.get_mut().set_len(0)?;
			reading.seek(std::io::SeekFrom::Start(0))?;
		}
		*self.reading_state.lock() = ReadingState::Reading;
		self.dirty.store(true, Ordering::Relaxed);
		Ok(())
	}

	pub fn begin_record<'a>(&'a self) -> LogWriter<'a> {
		let id = self.next_record_id.fetch_add(1, Ordering::Relaxed);
		let writer = LogWriter::new(
			&self.overlays,
			id
		);
		writer
	}

	pub fn end_record(&self, log: LogChange) -> Result<u64> {
		assert!(log.record_id + 1 == self.next_record_id.load(Ordering::Relaxed));
		let record_id = log.record_id;
		let mut appending = self.appending.write();
		let (index, values, bytes) = log.to_file(&mut appending.file)?;
		let mut overlays = self.overlays.write();
		let mut total_index = 0;
		for (id, overlay) in index.into_iter() {
			total_index += overlay.map.len();
			overlays.index.entry(id).or_default().map.extend(overlay.map.into_iter());
		}
		let mut total_value = 0;
		for (id, overlay) in values.into_iter() {
			total_value += overlay.map.len();
			overlays.value.entry(id).or_default().map.extend(overlay.map.into_iter());
		}
		log::debug!(
			target: "parity-db",
			"Finalizing log record {} ({} index, {} value)",
			record_id,
			total_index,
			total_value,
		);
		appending.empty = false;
		self.dirty.store(true, Ordering::Relaxed);
		Ok(bytes)
	}

	pub fn end_read(&self, cleared: Cleared, record_id: u64) {
		if record_id >= self.next_record_id.load(Ordering::Relaxed) {
			self.next_record_id.store(record_id + 1, Ordering::Relaxed);
		}
		let mut overlays = self.overlays.write();
		for (table, index) in cleared.index.into_iter() {
			if let Some(ref mut overlay) = overlays.index.get_mut(&table) {
				match overlay.map.entry(index) {
					std::collections::hash_map::Entry::Occupied(e) => {
						if e.get().0 == record_id {
							e.remove_entry();
						}
					}
					_ => {},
				}
			}
		}
		for (table, index) in cleared.values.into_iter() {
			if let Some(ref mut overlay) = overlays.value.get_mut(&table) {
				match overlay.map.entry(index) {
					std::collections::hash_map::Entry::Occupied(e) => {
						if e.get().0 == record_id {
							e.remove_entry();
						}
					}
					_ => {},
				}
			}
		}
		// Cleanup index overlays
		overlays.index.retain(|_, overlay| !overlay.map.is_empty());
	}

	pub fn flush_one(&self) -> Result<(bool, bool)> {
		// Wait for the reader to finish reading
		let mut flushing = self.flushing.lock();
		let mut read_next = false;
		if !flushing.empty {
			let mut reading_state = self.reading_state.lock();

			while *reading_state == ReadingState::Reading  {
				log::debug!(target: "parity-db", "Flush: Awaiting log reader");
				self.done_reading_cv.wait(&mut reading_state)
			}
			if *reading_state == ReadingState::Shutdown {
				return Ok((false, false))
			}

			{
				log::debug!(target: "parity-db", "Flush: Activated log reader");
				let mut reading = self.reading.write();
				std::mem::swap(reading.get_mut(), &mut flushing.file);
				reading.seek(std::io::SeekFrom::Start(0))?;
				*reading_state = ReadingState::Reading;
				read_next = true;
			}

			flushing.file.set_len(0)?;
			flushing.file.seek(std::io::SeekFrom::Start(0))?;
			flushing.empty = true;
		}

		let mut flush = false;
		{
			// Lock writer and reset it
			let mut appending = self.appending.write();
			if !appending.empty {
				std::mem::swap(appending.file.get_mut(), &mut flushing.file);
				flush = true;
				log::debug!(target: "parity-db", "Flush: Activated log writer");
				appending.empty = true;
			}
		}

		if flush {
			// Flush to disk
			log::debug!(target: "parity-db", "Flush: Flushing to disk");
			if self.sync {
				flushing.file.sync_data()?;
			}
			log::debug!(target: "parity-db", "Flush: Flushing completed");
			flushing.empty = false;
		}

		Ok((!flushing.empty, read_next))
	}

	pub fn read_next<'a>(&'a self) -> Result<Option<LogReader<'a>>> {
		let mut reading_state = self.reading_state.lock();
		if *reading_state != ReadingState::Reading {
			log::trace!(target: "parity-db", "No logs to enact");
			return Ok(None);
		}

		let reading = self.reading.write();
		let mut reader = LogReader::new(reading);
		match reader.next() {
			Ok(LogAction::BeginRecord(_)) => {
				return Ok(Some(reader));
			}
			Ok(_) => return Err(Error::Corruption("Bad log record structure".into())),
			Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
				*reading_state = ReadingState::Idle;
				self.done_reading_cv.notify_one();
				log::debug!(target: "parity-db", "Read: End of log");
				return Ok(None);
			}
			Err(e) => return Err(e),
		};
	}

	pub fn overlays(&self) -> &RwLock<LogOverlays> {
		&self.overlays
	}

	pub fn shutdown(&self) {
		let mut reading_state = self.reading_state.lock();
		*reading_state = ReadingState::Shutdown;
		self.done_reading_cv.notify_one();
	}
}
