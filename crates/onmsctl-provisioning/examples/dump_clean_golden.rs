/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! Regenerate the golden YAML fixture by running the clean-requisition
//! XML pair through the convert pipeline and dumping the result.
//!
//! Used to re-bless `tests/fixtures/convert/clean-requisition.expected.yaml`
//! when an intentional change to canonical output ships. After running,
//! commit the updated `.expected.yaml` alongside whatever code change
//! produced the new output.
//!
//! Usage:
//!
//! ```sh
//! cargo run --example dump_clean_golden -p onmsctl-provisioning \
//!     > crates/onmsctl-provisioning/tests/fixtures/convert/clean-requisition.expected.yaml
//! ```

use onmsctl_provisioning::convert::convert_requisition_xml;

const CLEAN_REQ: &str = include_str!("../tests/fixtures/convert/clean-requisition.xml");
const CLEAN_FS: &str = include_str!("../tests/fixtures/convert/clean-foreign-source.xml");

fn main() {
    let outcome =
        convert_requisition_xml(CLEAN_REQ, Some(CLEAN_FS), None).expect("clean fixture converts");
    let yaml = outcome.yaml.expect("YAML emitted");
    print!("{yaml}");
}
