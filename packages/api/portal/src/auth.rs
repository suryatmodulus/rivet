use api_helper::{
	auth::{ApiAuth, AuthRateLimitCtx},
	util::{as_auth_expired, basic_rate_limit},
};
use proto::claims::Claims;
use rivet_claims::ClaimsDecode;
use rivet_operation::prelude::*;

/// Information derived from the authentication middleware.
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

	pub async fn user(&self, ctx: &OperationContext<()>) -> GlobalResult<rivet_claims::ent::User> {
		let claims = self.claims()?;
		let user_ent = claims.as_user()?;

		let user_res = chirp_workflow::compat::op(
			&ctx,
			::user::ops::get::Input {
				user_ids: vec![user_ent.user_id],
			},
		)
		.await?;
		let Some(user) = user_res.users.first() else {
			bail_with!(TOKEN_REVOKED)
		};

		// Verify user is not deleted
		if user.delete_complete_ts.is_some() {
			let jti = unwrap!(claims.jti);
			op!([ctx] token_revoke {
				jtis: vec![jti],
			})
			.await?;

			bail_with!(TOKEN_REVOKED);
		}

		Ok(user_ent)
	}

	/// Validates that the given agent is an admin user
	pub async fn admin(&self, ctx: &OperationContext<()>) -> GlobalResult<()> {
		let user_ent = self.user(ctx).await?;

		// Get user
		let user_res = chirp_workflow::compat::op(
			&ctx,
			::user::ops::get::Input {
				user_ids: vec![user_ent.user_id],
			},
		)
		.await?;

		let user = unwrap!(user_res.users.first(), "user not found");

		ensure_with!(user.is_admin, IDENTITY_NOT_ADMIN);

		Ok(())
	}
}
