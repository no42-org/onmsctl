/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `ThreshdPackageHandler`: the router adapter for `kind: ThreshdPackage`.

use std::collections::HashSet;

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use super::{Planned, check_unique_names, join_diffs, write_failure};
use crate::api::{ThreshdApi, ThresholdingApi};
use crate::convert::{canonical_package, package_to_wire};
use crate::diff::package_diff;
use crate::dto::ThreshdPackageDto;
use crate::model::{GROUP_KIND, PACKAGE_KIND, ThreshdPackageLocal};

/// Handler for `kind: ThreshdPackage` documents.
#[derive(Default)]
pub struct ThreshdPackageHandler;

#[async_trait]
impl KindHandler for ThreshdPackageHandler {
    fn kind(&self) -> &'static str {
        PACKAGE_KIND
    }

    async fn plan(&self, docs: &[RawDoc], params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        let mut parsed = Vec::with_capacity(docs.len());
        for d in docs {
            parsed.push((d.label(), ThreshdPackageLocal::from_raw(d)?));
        }
        check_unique_names(
            PACKAGE_KIND,
            parsed
                .iter()
                .map(|(l, p)| (l.clone(), p.metadata.name.as_str())),
        )?;

        let client = OnmsClient::from_context(ctx)?;
        let groups_api = ThresholdingApi::new(&client);
        groups_api.metadata().await?;

        // Group gate: a group on the server (read-only ones included) or a
        // ThresholdGroup in this apply, which the group bucket creates first.
        let mut known: HashSet<String> = groups_api
            .list_groups()
            .await?
            .into_iter()
            .map(|g| g.name)
            .collect();
        known.extend(
            params
                .manifest
                .iter()
                .filter(|d| d.kind == GROUP_KIND)
                .filter_map(|d| d.name.clone()),
        );
        for (label, p) in &parsed {
            for (path, group) in p.bound_groups() {
                if !known.contains(group) {
                    return Err(Error::Config(format!(
                        "{label}: {path} '{group}' names no threshold group on the server or \
                         as a ThresholdGroup in this apply"
                    )));
                }
            }
        }

        let api = ThreshdApi::new(&client);
        let mut items = Vec::with_capacity(parsed.len());
        let mut preview = Vec::with_capacity(parsed.len());
        let mut diffs = Vec::new();
        for (_, local) in parsed {
            let name = local.metadata.name.clone();
            let live = api.get_package(&name).await?;
            let desired = package_to_wire(&local, live.as_ref().map(|(p, _)| p));
            let (action, etag) = match live {
                None => {
                    diffs.push(package_diff(None, &canonical_package(&desired)));
                    (Action::Create, None)
                }
                Some((live, etag)) => {
                    let (live, want) = (canonical_package(&live), canonical_package(&desired));
                    if live == want {
                        (Action::None, etag)
                    } else {
                        diffs.push(package_diff(Some(&live), &want));
                        (Action::Update, etag)
                    }
                }
            };
            preview.push(ApplyOutcome::would(PACKAGE_KIND, name.clone(), action));
            items.push(Planned {
                name,
                desired,
                action,
                etag,
            });
        }
        Ok(Plan::new(preview, Box::new(items)).with_diff(join_diffs(diffs)))
    }

    async fn execute(
        &self,
        plan: Plan,
        _params: &ApplyParams,
        ctx: &Context,
    ) -> Result<Vec<ApplyOutcome>> {
        let items = plan
            .payload
            .downcast::<Vec<Planned<ThreshdPackageDto>>>()
            .map_err(|_| {
                Error::Config("internal: ThreshdPackageHandler payload type mismatch".into())
            })?;
        let client = OnmsClient::from_context(ctx)?;
        let api = ThreshdApi::new(&client);
        let mut out = Vec::with_capacity(items.len());
        for it in *items {
            let result = match it.action {
                Action::Create => api
                    .create_package(&it.desired)
                    .await
                    .map(|()| (OutcomeStatus::Created, "created")),
                Action::Update => api
                    .update_package(&it.name, &it.desired, it.etag.as_deref())
                    .await
                    .map(|()| (OutcomeStatus::Updated, "updated")),
                _ => Ok((OutcomeStatus::Unchanged, "in sync")),
            };
            out.push(match result {
                Ok((status, msg)) => {
                    ApplyOutcome::new(PACKAGE_KIND, it.name, it.action, status, msg)
                }
                Err(e) => write_failure(PACKAGE_KIND, &it.name, it.action, e),
            });
        }
        Ok(out)
    }
}
