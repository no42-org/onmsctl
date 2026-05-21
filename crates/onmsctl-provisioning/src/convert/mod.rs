/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! XML→YAML migrator for OpenNMS `provision.pl`-shape inputs.
//!
//! The migrator reads requisition XML (matching the wire shape of
//! `GET /rest/requisitions/{fs}`) and optional foreign-source XML
//! (matching `etc/foreign-sources/*.xml`), and emits a composite
//! `kind: Requisition` YAML document suitable for
//! `onmsctl requisition apply -f`.
//!
//! Findings are reported with stable `PR###` codes so operators can
//! grep, count, and `--explain` them. Codes are namespaced under `PR`
//! (provisioning) to keep them visually distinct from eventconf's
//! `EC###` catalog. Reserved space: PR001–PR099.
//!
//! Module layout:
//!
//! - [`finding`]: `FindingCode` enum + `Finding` record + `Severity`
//! - [`xml`]: serde DTOs for `<model-import>` (requisition) and
//!   `<foreign-source>` XML files
//! - [`pipeline`]: end-to-end conversion that ties XML readers,
//!   findings, and YAML emission together
//!
//! Today (tasks 8.1–8.4) only the first four `PR###` codes are
//! emitted by the pipeline; more land as fixture work in 8.6–8.7
//! surfaces additional cases.

pub mod finding;
pub mod pipeline;
pub mod xml;

pub use finding::{Finding, FindingCode, Severity, explain};
pub use pipeline::{ConversionResult, convert_directory, convert_requisition_xml};
