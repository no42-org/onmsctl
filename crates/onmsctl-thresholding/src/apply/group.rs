/*
 * Copyright 2026 Ronny Trommer <ronny@no42.org>
 * SPDX-License-Identifier: Apache-2.0
 */

//! `ThresholdGroupHandler`: the router adapter for `kind: ThresholdGroup`.

use std::collections::HashSet;

use async_trait::async_trait;

use onmsctl_core::{
    Action, ApplyOutcome, ApplyParams, Context, Error, KindHandler, OnmsClient, OutcomeStatus,
    Plan, RawDoc, Result,
};

use super::{Planned, check_unique_names, join_diffs, write_failure};
use crate::api::ThresholdingApi;
use crate::convert::{canonical_group, group_to_wire};
use crate::diff::group_diff;
use crate::dto::ThresholdGroupDto;
use crate::model::{GROUP_KIND, ThresholdGroupLocal};

/// Handler for `kind: ThresholdGroup` documents.
#[derive(Default)]
pub struct ThresholdGroupHandler;

#[async_trait]
impl KindHandler for ThresholdGroupHandler {
    fn kind(&self) -> &'static str {
        GROUP_KIND
    }

    async fn plan(&self, docs: &[RawDoc], _params: &ApplyParams, ctx: &Context) -> Result<Plan> {
        let mut parsed = Vec::with_capacity(docs.len());
        for d in docs {
            parsed.push((d.label(), ThresholdGroupLocal::from_raw(d)?));
        }
        check_unique_names(
            GROUP_KIND,
            parsed
                .iter()
                .map(|(l, g)| (l.clone(), g.metadata.name.as_str())),
        )?;

        let client = OnmsClient::from_context(ctx)?;
        let api = ThresholdingApi::new(&client);
        let metadata = api.metadata().await?;

        // dsType gate: the server accepts unknown datasource types, so this
        // is the only place a typo is caught before thresholding goes quiet.
        let valid: HashSet<&str> = metadata.ds_types.iter().map(|t| t.name.as_str()).collect();
        for (label, g) in &parsed {
            for (path, ds_type) in g.ds_types() {
                if !valid.contains(ds_type) {
                    return Err(Error::Config(format!(
                        "{label}: {path}.dsType '{ds_type}' is not a datasource type on this \
                         server ({} types, e.g. node, if; see GET /api/v2/thresholding/metadata)",
                        valid.len()
                    )));
                }
            }
        }

        let mut items = Vec::with_capacity(parsed.len());
        let mut preview = Vec::with_capacity(parsed.len());
        let mut diffs = Vec::new();
        for (label, local) in parsed {
            let name = local.metadata.name.clone();
            let desired = group_to_wire(&local);
            let (action, etag) = match api.get_group(&name).await? {
                None => {
                    diffs.push(group_diff(None, &canonical_group(&desired)));
                    (Action::Create, None)
                }
                Some((live, _)) if live.read_only => {
                    return Err(Error::Config(format!(
                        "{label}: threshold group '{name}' is contributed by an extension and \
                         cannot be managed with apply"
                    )));
                }
                Some((live, etag)) => {
                    let (live, want) = (canonical_group(&live), canonical_group(&desired));
                    if live == want {
                        (Action::None, etag)
                    } else {
                        diffs.push(group_diff(Some(&live), &want));
                        (Action::Update, etag)
                    }
                }
            };
            preview.push(ApplyOutcome::would(GROUP_KIND, name.clone(), action));
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
            .downcast::<Vec<Planned<ThresholdGroupDto>>>()
            .map_err(|_| {
                Error::Config("internal: ThresholdGroupHandler payload type mismatch".into())
            })?;
        let client = OnmsClient::from_context(ctx)?;
        let api = ThresholdingApi::new(&client);
        let mut out = Vec::with_capacity(items.len());
        for it in *items {
            let result = match it.action {
                Action::Create => api
                    .create_group(&it.desired)
                    .await
                    .map(|()| (OutcomeStatus::Created, "created")),
                Action::Update => api
                    .update_group(&it.name, &it.desired, it.etag.as_deref())
                    .await
                    .map(|()| (OutcomeStatus::Updated, "updated")),
                _ => Ok((OutcomeStatus::Unchanged, "in sync")),
            };
            out.push(match result {
                Ok((status, msg)) => ApplyOutcome::new(GROUP_KIND, it.name, it.action, status, msg),
                Err(e) => write_failure(GROUP_KIND, &it.name, it.action, e),
            });
        }
        Ok(out)
    }
}
