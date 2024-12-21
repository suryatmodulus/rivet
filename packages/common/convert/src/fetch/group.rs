use rivet_api::models;
use rivet_operation::prelude::*;

use crate::convert;

pub async fn summaries(
	ctx: &OperationContext<()>,
	current_user_id: Option<Uuid>,
	group_ids: Vec<Uuid>,
) -> GlobalResult<Vec<models::GroupGroupSummary>> {
	if group_ids.is_empty() {
		return Ok(Vec::new());
	}

	let group_ids_proto = group_ids
		.clone()
		.into_iter()
		.map(Into::into)
		.collect::<Vec<_>>();

	// Fetch team metadata
	let (user_teams, teams_res, team_member_count_res) = tokio::try_join!(
		async {
			if let Some(current_user_id) = current_user_id {
				let user_team_list_res = chirp_workflow::compat::op(
					&ctx,
					::user::ops::team_list::Input {
						user_ids: vec![current_user_id.into()],
					},
				)
				.await?;

				Ok(unwrap!(user_team_list_res.users.first()).teams.clone())
			} else {
				Ok(Vec::new())
			}
		},
		op!([ctx] team_get {
			team_ids: group_ids_proto.clone(),
		}),
		op!([ctx] team_member_count {
			team_ids: group_ids_proto.clone(),
		}),
	)?;

	teams_res
		.teams
		.iter()
		.map(|team| {
			let is_current_identity_member = user_teams
				.iter()
				.any(|t| Some(common::Uuid::from(t.team_id)) == team.team_id);

			convert::group::summary(
				ctx.config(),
				team,
				&team_member_count_res.teams,
				is_current_identity_member,
			)
		})
		.collect::<GlobalResult<Vec<_>>>()
}
