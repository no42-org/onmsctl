/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! The `kind: DataCollectionSource` apply handler (per-source composite
//! reconcile). See [`handler`] for the plan/execute logic.

mod handler;

pub use handler::DataCollectionSourceHandler;
