// SPDX-License-Identifier: MPL-2.0

//! Independent Workspace admissions with bounded active threads and persistent subscriptions.
//! A completed first pass hands the stream to the host's retained subscription;
//! it does not hold the lane forever. Only this owner admits, renews and fences
//! permits. Admission threads are joined; the host owns live stream cleanup.
use super::*;

struct Lane {
    pending: VecDeque<Admission>,
    current: Option<Admission>,
    workspace: bool,
    attempt: u8,
    running: Option<std::thread::JoinHandle<()>>,
}

impl<A: Api, H: Host> Coordinator<A, H> {
    pub(super) fn run_sessions(
        &mut self,
        admitted: Vec<Admission>,
        clock: &dyn Clock,
        failure: &mut Option<Failure>,
    ) -> bool {
        let mut order = VecDeque::new();
        let mut lanes = BTreeMap::<(ProfileId, WorkspaceId), Lane>::new();
        for admission in admitted {
            order.push_back((
                admission.candidate.profile_id,
                admission.candidate.workspace_id,
            ));
            lanes
                .entry((
                    admission.candidate.profile_id,
                    admission.candidate.workspace_id,
                ))
                .or_insert_with(|| Lane {
                    pending: VecDeque::new(),
                    current: None,
                    workspace: true,
                    attempt: 0,
                    running: None,
                })
                .pending
                .push_back(admission);
        }
        let (sender, receiver) = mpsc::sync_channel(lanes.len().max(1));
        let mut hinted = false;
        loop {
            let mut running = lanes.values().filter(|lane| lane.running.is_some()).count();
            for _ in 0..order.len() {
                if running >= COHORT_WORKSPACES {
                    break;
                }
                let key = order.pop_front().expect("admission order");
                order.push_back(key);
                let lane = lanes.get_mut(&key).expect("admission lane");
                if lane.running.is_some() {
                    continue;
                }
                loop {
                    if lane.current.is_none() {
                        lane.current = lane.pending.pop_front();
                        lane.workspace = true;
                        lane.attempt = 0;
                    }
                    let Some(admission) = lane.current.as_mut() else {
                        break;
                    };
                    let prepared =
                        self.prepare_session(admission, lane.workspace, lane.attempt, clock);
                    let result = prepared.and_then(|()| {
                        self.spawn_session(
                            admission,
                            lane.workspace,
                            lane.attempt,
                            key,
                            sender.clone(),
                        )
                    });
                    match result {
                        Ok(handle) => {
                            lane.running = Some(handle);
                            running += 1;
                            break;
                        }
                        Err(error) => self.finish_session(lane, Err(error), failure, &mut hinted),
                    }
                }
            }
            if lanes.values().all(|lane| lane.running.is_none()) {
                if lanes
                    .values()
                    .all(|lane| lane.current.is_none() && lane.pending.is_empty())
                {
                    break;
                }
                continue;
            }
            // Every running session is bounded by its host and observes the
            // shared authorization. One blocked Profile cannot hold another's
            // next Workspace behind a cohort-wide join.
            let Ok((key, result)) = receiver.recv() else {
                break;
            };
            let lane = lanes.get_mut(&key).expect("owned Workspace lane");
            if let Some(handle) = lane.running.take() {
                let _ = handle.join();
            }
            let expired = matches!(&result, Err(Failure::CredentialExpired))
                || result
                    .as_ref()
                    .is_ok_and(|report| report.error_code == Some(CREDENTIAL_EXPIRED));
            if expired && lane.attempt == 0 {
                lane.attempt = 1;
            } else {
                self.finish_session(lane, result, failure, &mut hinted);
            }
        }
        hinted
    }

    fn spawn_session(
        &self,
        admission: &Admission,
        workspace: bool,
        attempt: u8,
        key: (ProfileId, WorkspaceId),
        done: mpsc::SyncSender<((ProfileId, WorkspaceId), Result<SessionReport, Failure>)>,
    ) -> Result<std::thread::JoinHandle<()>, Failure> {
        let authority = self.current_authority()?;
        let access = self.session_access(&authority);
        let keys = self.jwks.as_ref().ok_or(Failure::Unavailable)?.1.clone();
        let host = self.host.clone();
        let token = admission.token.clone();
        let permit = if workspace {
            &admission.workspace_permit
        } else {
            &admission.profile_permit
        }
        .clone();
        let route = if workspace {
            token.workspace()
        } else {
            token.profile()
        }
        .clone();
        let profile = admission.candidate.profile_id;
        tracing::info!(?profile, workspace = ?admission.candidate.workspace_id,
            dataset = ?permit.dataset, attempt, "cloud session: starting Profile lane");
        std::thread::Builder::new()
            .name("koru-cloud-profile".into())
            .spawn(move || {
                // Always report completion, including a port panic. The owner
                // joins this handle before admitting the Profile's next session.
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match route {
                        DatasetRouteResultV1::Unavailable { .. } => Err(Failure::WorkerUnavailable),
                        DatasetRouteResultV1::Available { dataset, route } => host.open_dataset(
                            &permit,
                            token.credential(),
                            &route,
                            dataset,
                            &keys,
                            &access,
                        ),
                    }))
                    .unwrap_or(Err(Failure::Unavailable));
                let _ = done.send((key, result));
            })
            .map_err(|_| Failure::Unavailable)
    }

    fn finish_session(
        &mut self,
        lane: &mut Lane,
        result: Result<SessionReport, Failure>,
        failure: &mut Option<Failure>,
        hinted: &mut bool,
    ) {
        let admission = lane.current.as_ref().expect("active admission");
        let permit = if lane.workspace {
            &admission.workspace_permit
        } else {
            &admission.profile_permit
        };
        match result {
            Ok(report) => {
                tracing::info!(profile = ?permit.profile_id, dataset = ?permit.dataset,
                    stage = ?report.stage, code = report.error_code, "cloud session: Profile lane answered");
                *hinted |= report.remote_hint;
                if let Some(record) = self.records.get_mut(&permit.permit_id) {
                    record.stage = report.stage;
                    record.error_code = report.error_code;
                    record.bytes_held = report.bytes_held;
                    record.queued_jobs = report.queued_jobs;
                    record.pending_consent_ids = report.pending_consent_ids;
                    record.pending_consent_ids.truncate(2);
                }
                if report.error_code == Some(CREDENTIAL_EXPIRED) {
                    *failure = Some(merge(failure.take(), Failure::CredentialExpired));
                }
            }
            Err(error) => {
                if let Some(record) = self.records.get_mut(&permit.permit_id) {
                    record.stage = stage_for(&error);
                    record.error_code = Some(error.code());
                }
                *failure = Some(merge(failure.take(), error));
            }
        }
        if lane.workspace {
            lane.workspace = false;
        } else {
            self.admissions
                .insert(admission.candidate.workspace_id, admission.clone());
            lane.current.take();
        }
        lane.attempt = 0;
        self.publish_progress();
    }
}
