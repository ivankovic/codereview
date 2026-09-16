/*  This file is part of codereview.
 *
 *  Copyright (C) 2026 Marko Ivankovic
 *
 *  This program is free software: you can redistribute it and/or modify
 *  it under the terms of the GNU Affero General Public License as published
 *  by the Free Software Foundation, either version 3 of the License, or
 *  (at your option) any later version.
 *
 *  This program is distributed in the hope that it will be useful,
 *  but WITHOUT ANY WARRANTY; without even the implied warranty of
 *  MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 *  GNU Affero General Public License for more details.
 *
 *  You should have received a copy of the GNU Affero General Public License
 *  along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

//! Explore and review fast-changing codebases. See PLAN.md for the shape of the crate.

pub mod acp;
pub mod agent;
pub mod anchor;
pub mod claude;
pub mod config;
pub mod diff;
#[cfg(test)]
mod fakes;
pub mod highlight;
pub mod markdown;
pub mod notes;
pub mod repo;
pub mod review;
pub mod session;
pub mod symbols;
pub mod theme;
pub mod transcript;

#[cfg(feature = "tui")]
pub mod tui;
#[cfg(feature = "web")]
pub mod web;
