use api_helper::ctx::Ctx;
use rivet_group_server::models;
use rivet_operation::prelude::*;

use crate::{auth::Auth, convert};

pub async fn summaries(
	ctx: &Ctx<Auth>,
	current_user_id: Uuid,
	group_ids: Vec<Uuid>,
) -> GlobalResult<Vec<models::GroupSummary>> {
	let group_ids_proto = group_ids
		.clone()
		.into_iter()
		.map(Into::into)
		.collect::<Vec<_>>();

	// Fetch team metadata
	let (user_team_list_res, teams_res, team_member_count_res) = tokio::try_join!(
		(*ctx).op(
			::user::ops::team_list::Input {
				user_ids: vec![current_user_id.into()],
			}
		),
		op!([ctx] team_get {
			team_ids: group_ids_proto.clone(),
		}),
		op!([ctx] team_member_count {
			team_ids: group_ids_proto.clone(),
		}),
	)?;

	let user_teams = &unwrap!(user_team_list_res.users.first()).teams;

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
		.collect::<GlobalResult<Vec<models::GroupSummary>>>()
}
