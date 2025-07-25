use global_error::{GlobalError, GlobalResult};
use rivet_pools::prelude::*;
use serde::Serialize;
use tracing::Instrument;
use uuid::Uuid;

use crate::{
	builder::{common as builder, WorkflowRepr},
	ctx::{
		common,
		message::{SubscriptionHandle, TailAnchor, TailAnchorResponse},
		MessageCtx,
	},
	db::DatabaseHandle,
	error::WorkflowResult,
	message::{Message, NatsMessage},
	operation::{Operation, OperationInput},
	signal::Signal,
	utils::tags::AsTags,
	workflow::{Workflow, WorkflowInput},
};

#[derive(Clone)]
pub struct StandaloneCtx {
	ray_id: Uuid,
	name: &'static str,
	ts: i64,

	db: DatabaseHandle,

	config: rivet_config::Config,
	conn: rivet_connection::Connection,
	msg_ctx: MessageCtx,

	// Backwards compatibility
	op_ctx: rivet_operation::OperationContext<()>,
}

impl StandaloneCtx {
	#[tracing::instrument(skip_all, fields(name, ray_id, req_id))]
	pub async fn new(
		db: DatabaseHandle,
		config: rivet_config::Config,
		conn: rivet_connection::Connection,
		name: &'static str,
	) -> WorkflowResult<Self> {
		let req_id = Uuid::new_v4();
		let ray_id = Uuid::new_v4();
		let ts = rivet_util::timestamp::now();

		let span = tracing::Span::current();
		span.record("req_id", req_id.to_string());
		span.record("ray_id", ray_id.to_string());

		let op_ctx = rivet_operation::OperationContext::new(
			name.to_string(),
			std::time::Duration::from_secs(60),
			config.clone(),
			conn.clone(),
			req_id,
			ray_id,
			ts,
			ts,
			(),
		);

		let msg_ctx = MessageCtx::new(&conn, ray_id).await?;

		Ok(StandaloneCtx {
			ray_id,
			name,
			ts,
			db,
			config,
			conn,
			op_ctx,
			msg_ctx,
		})
	}
}

impl StandaloneCtx {
	// TODO: Should be T: Workflow instead of I: WorkflowInput but rust can't figure it out
	/// Creates a workflow builder.
	pub fn workflow<I>(
		&self,
		input: impl WorkflowRepr<I>,
	) -> builder::workflow::WorkflowBuilder<impl WorkflowRepr<I>, I>
	where
		I: WorkflowInput,
		<I as WorkflowInput>::Workflow: Workflow<Input = I>,
	{
		builder::workflow::WorkflowBuilder::new(self.db.clone(), self.ray_id, input, false)
	}

	/// Creates a signal builder.
	pub fn signal<T: Signal + Serialize>(&self, body: T) -> builder::signal::SignalBuilder<T> {
		builder::signal::SignalBuilder::new(self.db.clone(), self.ray_id, body, false)
	}

	#[tracing::instrument(skip_all, fields(operation_name=I::Operation::NAME))]
	pub async fn op<I>(
		&self,
		input: I,
	) -> GlobalResult<<<I as OperationInput>::Operation as Operation>::Output>
	where
		I: OperationInput,
		<I as OperationInput>::Operation: Operation<Input = I>,
	{
		common::op(
			&self.db,
			&self.config,
			&self.conn,
			self.ray_id,
			self.op_ctx.req_ts(),
			false,
			input,
		)
		.in_current_span()
		.await
	}

	/// Creates a message builder.
	pub fn msg<M: Message>(&self, body: M) -> builder::message::MessageBuilder<M> {
		builder::message::MessageBuilder::new(self.msg_ctx.clone(), body)
	}

	#[tracing::instrument(skip_all, fields(message=M::NAME))]
	pub async fn subscribe<M>(&self, tags: impl AsTags) -> GlobalResult<SubscriptionHandle<M>>
	where
		M: Message,
	{
		self.msg_ctx
			.subscribe::<M>(tags)
			.in_current_span()
			.await
			.map_err(GlobalError::raw)
	}

	#[tracing::instrument(skip_all, fields(message=M::NAME))]
	pub async fn tail_read<M>(&self, tags: impl AsTags) -> GlobalResult<Option<NatsMessage<M>>>
	where
		M: Message,
	{
		self.msg_ctx
			.tail_read::<M>(tags)
			.in_current_span()
			.await
			.map_err(GlobalError::raw)
	}

	#[tracing::instrument(skip_all, fields(message=M::NAME))]
	pub async fn tail_anchor<M>(
		&self,
		tags: impl AsTags,
		anchor: &TailAnchor,
	) -> GlobalResult<TailAnchorResponse<M>>
	where
		M: Message,
	{
		self.msg_ctx
			.tail_anchor::<M>(tags, anchor)
			.in_current_span()
			.await
			.map_err(GlobalError::raw)
	}
}

impl StandaloneCtx {
	pub fn name(&self) -> &str {
		self.name
	}

	// pub fn timeout(&self) -> Duration {
	// 	self.timeout
	// }

	pub fn req_id(&self) -> Uuid {
		self.op_ctx.req_id()
	}

	pub fn ray_id(&self) -> Uuid {
		self.ray_id
	}

	/// Timestamp at which the request started.
	pub fn ts(&self) -> i64 {
		self.ts
	}

	/// Timestamp at which the request was published.
	pub fn req_ts(&self) -> i64 {
		self.op_ctx.req_ts()
	}

	/// Time between when the timestamp was processed and when it was published.
	pub fn req_dt(&self) -> i64 {
		self.ts.saturating_sub(self.op_ctx.req_ts())
	}

	pub fn config(&self) -> &rivet_config::Config {
		&self.config
	}

	pub fn trace(&self) -> &[chirp_client::TraceEntry] {
		self.conn.trace()
	}

	pub fn chirp(&self) -> &chirp_client::Client {
		self.conn.chirp()
	}

	pub fn cache(&self) -> rivet_cache::RequestConfig {
		self.conn.cache()
	}

	pub fn cache_handle(&self) -> rivet_cache::Cache {
		self.conn.cache_handle()
	}

	#[tracing::instrument(skip_all)]
	pub async fn crdb(&self) -> Result<CrdbPool, rivet_pools::Error> {
		self.conn.crdb().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn redis_chirp(&self) -> Result<RedisPool, rivet_pools::Error> {
		self.conn.redis_chirp().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn redis_cache(&self) -> Result<RedisPool, rivet_pools::Error> {
		self.conn.redis_cache().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn redis_cdn(&self) -> Result<RedisPool, rivet_pools::Error> {
		self.conn.redis_cdn().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn redis_job(&self) -> Result<RedisPool, rivet_pools::Error> {
		self.conn.redis_job().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn redis_mm(&self) -> Result<RedisPool, rivet_pools::Error> {
		self.conn.redis_mm().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn clickhouse(&self) -> GlobalResult<ClickHousePool> {
		self.conn.clickhouse().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn clickhouse_inserter(&self) -> GlobalResult<ClickHouseInserterHandle> {
		self.conn.clickhouse_inserter().await
	}

	#[tracing::instrument(skip_all)]
	pub async fn fdb(&self) -> Result<FdbPool, rivet_pools::Error> {
		self.conn.fdb().await
	}

	#[tracing::instrument(skip_all, fields(%workflow_id))]
	pub async fn sqlite_for_workflow(&self, workflow_id: Uuid) -> GlobalResult<SqlitePool> {
		common::sqlite_for_workflow(&self.db, &self.conn, workflow_id, true)
			.in_current_span()
			.await
	}

	// Backwards compatibility
	pub fn op_ctx(&self) -> &rivet_operation::OperationContext<()> {
		&self.op_ctx
	}
}
