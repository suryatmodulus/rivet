use api_helper::{
	auth::{ApiAuth, AuthRateLimitCtx},
	util::{as_auth_expired, basic_rate_limit},
};
use proto::claims::Claims;
use rivet_claims::ClaimsDecode;
use rivet_operation::prelude::*;

pub struct Auth {
	config: rivet_config::Config,
	claims: Option<Claims>,
}

#[async_trait]
impl ApiAuth for Auth {
	async fn new(
		config: rivet_config::Config,
		api_token: Option<String>,
		rate_limit_ctx: AuthRateLimitCtx<'_>,
	) -> GlobalResult<Auth> {
		Self::rate_limit(&config, rate_limit_ctx).await?;

		Ok(Auth {
			config: config.clone(),
			claims: if let Some(api_token) = api_token {
				Some(as_auth_expired(rivet_claims::decode(
					&config.server()?.jwt.public,
					&api_token,
				)?)?)
			} else {
				None
			},
		})
	}

	async fn rate_limit(
		config: &rivet_config::Config,
		rate_limit_ctx: AuthRateLimitCtx<'_>,
	) -> GlobalResult<()> {
		basic_rate_limit(config, rate_limit_ctx).await
	}
}

impl Auth {
	pub fn claims(&self) -> GlobalResult<&Claims> {
		self.claims
			.as_ref()
			.ok_or_else(|| err_code!(API_UNAUTHORIZED, reason = "No bearer token provided."))
	}

	pub fn server(&self) -> GlobalResult<rivet_claims::ent::EnvService> {
		self.claims()?.as_env_service()
	}

	pub async fn check_game(
		&self,
		ctx: &OperationContext<()>,
		game_id: Uuid,
		env_id: Uuid,
		allow_service: bool,
	) -> GlobalResult<()> {
		let claims = self.claims()?;

		// Get the game this env belongs to
		let ns_res = op!([ctx] game_namespace_get {
			namespace_ids: vec![env_id.into()],
		})
		.await?;
		let env = unwrap_with!(ns_res.namespaces.first(), ENVIRONMENT_NOT_FOUND);

		// Ensure belongs to game
		ensure_with!(
			unwrap!(env.game_id).as_uuid() == game_id,
			ENVIRONMENT_NOT_FOUND
		);

		// Validate token
		if let Ok(cloud_ent) = claims.as_game_cloud() {
			ensure_with!(
				cloud_ent.game_id == game_id,
				API_FORBIDDEN,
				reason = "Cloud token cannot write to this game",
			);
			Ok(())
		} else if let Ok(service_ent) = claims.as_env_service() {
			ensure_with!(
				allow_service,
				API_FORBIDDEN,
				reason = "Cannot use service token for this endpoint."
			);
			ensure_with!(
				service_ent.env_id == env_id,
				API_FORBIDDEN,
				reason = "Service token cannot write to this game",
			);
			Ok(())
		} else if let Ok(user_ent) = claims.as_user() {
			// Get the user
			let (user_res, game_res, team_list_res) = tokio::try_join!(
				op!([ctx] user_get {
					user_ids: vec![user_ent.user_id.into()],
				}),
				op!([ctx] game_get {
					game_ids: vec![game_id.into()],
				}),
				chirp_workflow::compat::op(
					&ctx,
					::user::ops::team_list::Input {
						user_ids: vec![user_ent.user_id.into()],
					},
				)
			)?;
			let Some(user) = user_res.users.first() else {
				bail_with!(TOKEN_REVOKED)
			};
			let game = unwrap_with!(game_res.games.first(), PROJECT_NOT_FOUND);
			let user_teams = unwrap!(team_list_res.users.first());
			let dev_team_id = unwrap_ref!(game.developer_team_id).as_uuid();

			// Allow admin
			if user.is_admin {
				return Ok(());
			}

			// Verify user is not deleted
			ensure_with!(user.delete_complete_ts.is_none(), TOKEN_REVOKED);

			// Validate user is member of team
			let is_part_of_team = user_teams
				.teams
				.iter()
				.any(|x| x.team_id == dev_team_id);
			ensure_with!(is_part_of_team, GROUP_NOT_MEMBER);

			// Get team
			let team_res = op!([ctx] team_get {
				team_ids: vec![dev_team_id.into()],
			})
			.await?;
			let dev_team = unwrap!(team_res.teams.first());

			// Check team active
			ensure_with!(
				dev_team.deactivate_reasons.is_empty(),
				GROUP_DEACTIVATED,
				reasons = util_team::format_deactivate_reasons(&dev_team.deactivate_reasons)?,
			);

			Ok(())
		} else {
			bail_with!(
				CLAIMS_MISSING_ENTITLEMENT,
				entitlements = "User, GameCloud, EnvService"
			);
		}
	}
}
