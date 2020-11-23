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

use std::fmt;
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
	Io(std::io::Error),
	Corruption(String),
	InvalidConfiguration(String),
	InvalidInput(String),
	Background(Arc<Error>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Io(e) => write!(f, "IO Error: {}", e),
			Error::Corruption(e) => write!(f, "Corruption: {}", e),
			Error::InvalidConfiguration(e) => write!(f, "Invalid configuration: {}", e),
			Error::InvalidInput(e) => write!(f, "Invalid input: {}", e),
			Error::Background(e) => write!(f, "Background worker error: {}", e),
		}
    }
}

impl From<std::io::Error> for Error {
	fn from(e: std::io::Error) -> Self {
		Error::Io(e)
	}
}
