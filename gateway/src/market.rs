//! Funded capacity auctions with uniform-price clearing and daily inference entitlements.
use crate::{
    Shared,
    billing::{Charge, Receipt},
    catalog::Route,
    control,
    error::ApiError,
    ledger,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn uuid(value: &Value, name: &str) -> Result<Uuid, ApiError> {
    value.as_str().and_then(|s| s.parse().ok()).ok_or_else(|| ApiError::bad_request(format!("{name} must be a UUID.")))
}
fn money(units: i64, days: i32, price: i64) -> Result<i64, ApiError> {
    i64::try_from(units as i128 * days as i128 * price as i128)
        .map_err(|_| ApiError::bad_request("Order notional is too large."))
}

pub async fn publish(
    pool: &PgPool,
    route: &Route,
    units: i64,
    days: i32,
    reserve: i64,
    duration: i64,
) -> anyhow::Result<Uuid> {
    anyhow::ensure!(
        (1..=1_000_000).contains(&units) && [7, 30, 90].contains(&days),
        "Use 1–1,000,000 IU/day and a 7, 30 or 90-day term"
    );
    anyhow::ensure!(
        reserve > 0 && reserve as u64 >= route.micro_usd_per_iu,
        "Reserve price must cover the route's current output-token cost per IU"
    );
    anyhow::ensure!((1..=604800).contains(&duration), "Auction duration must be 1 second to 7 days");
    money(units, days, reserve).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    anyhow::ensure!(
        (units as u128 * route.tokens_per_iu as u128 * 10_000) <= i64::MAX as u128,
        "Daily token capacity is too large"
    );
    anyhow::ensure!(
        route.tokens_per_iu <= i64::MAX as u64 && route.prompt_weight_bps <= i32::MAX as u32,
        "Route token units or input weight exceed storage limits"
    );
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO market_auctions(id,route_id,units,term_days,reserve_price_micro,tokens_per_iu,prompt_weight_bps,ends_at) VALUES($1,$2,$3,$4,$5,$6,$7,now()+make_interval(secs=>$8))")
        .bind(id).bind(&route.id).bind(units).bind(days).bind(reserve).bind(route.tokens_per_iu as i64).bind(route.prompt_weight_bps as i32).bind(duration as f64).execute(pool).await?;
    Ok(id)
}

pub async fn auctions(State(state): State<Shared>) -> Result<Json<Value>, ApiError> {
    let rows=sqlx::query("SELECT a.*,a.ends_at>now() AS accepting,COALESCE((SELECT sum(b.units) FROM market_bids b WHERE b.auction_id=a.id AND b.status='open'),0)::bigint AS bid_units,(SELECT count(*) FROM market_bids b WHERE b.auction_id=a.id AND b.status='open') AS bid_count FROM market_auctions a WHERE a.ends_at>now()-interval '7 days' ORDER BY a.created_at DESC LIMIT 200").fetch_all(&state.db).await?;
    let auctions=rows.into_iter().map(|r|Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"route_id":r.try_get::<String,_>("route_id")?,"units":r.try_get::<i64,_>("units")?,"term_days":r.try_get::<i32,_>("term_days")?,"reserve_price_micro_usd":r.try_get::<i64,_>("reserve_price_micro")?,"ends_at":r.try_get::<chrono::DateTime<chrono::Utc>,_>("ends_at")?,"status":r.try_get::<String,_>("status")?,"accepting":r.try_get::<bool,_>("accepting")?,"bid_units":r.try_get::<i64,_>("bid_units")?,"bid_count":r.try_get::<i64,_>("bid_count")?,"filled_units":r.try_get::<i64,_>("filled_units")?,"clearing_price_micro_usd":r.try_get::<Option<i64>,_>("clearing_price_micro")?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(json!({"auctions":auctions})))
}

pub async fn bids(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = control::user(&state, &headers)?;
    let rows=sqlx::query("SELECT b.*,a.route_id,a.term_days,a.ends_at FROM market_bids b JOIN accounts c ON c.id=b.account_id JOIN market_auctions a ON a.id=b.auction_id WHERE c.owner_user_id=$1 ORDER BY b.created_at DESC LIMIT 100").bind(user).fetch_all(&state.db).await?;
    let data=rows.into_iter().map(|r|Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"auction_id":r.try_get::<Uuid,_>("auction_id")?,"route_id":r.try_get::<String,_>("route_id")?,"units":r.try_get::<i64,_>("units")?,"term_days":r.try_get::<i32,_>("term_days")?,"max_price_micro_usd":r.try_get::<i64,_>("max_price_micro")?,"status":r.try_get::<String,_>("status")?,"filled_units":r.try_get::<i64,_>("filled_units")?,"charged_micro_usd":r.try_get::<i64,_>("charged_micro")?,"ends_at":r.try_get::<chrono::DateTime<chrono::Utc>,_>("ends_at")?}))).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(json!({"bids":data})))
}

pub async fn place_bid(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let user = control::user(&state, &headers)?;
    let account = control::owned_account(&state, &user).await?;
    let auction = uuid(&body["auction_id"], "auction_id")?;
    let client = uuid(&body["client_order_id"], "client_order_id")?;
    let units = body["units"]
        .as_i64()
        .filter(|n| (1..=1_000_000).contains(n))
        .ok_or_else(|| ApiError::bad_request("Quantity must be 1–1,000,000 IU/day."))?;
    let price = crate::api::account::parse_usd(&body["max_price_usd"])
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .ok_or_else(|| ApiError::bad_request("Set a positive maximum price in USD per IU-day."))?
        as i64;
    let mut tx = state.db.begin().await?;
    let lot = sqlx::query("SELECT *,ends_at>now() AS accepting FROM market_auctions WHERE id=$1 FOR UPDATE")
        .bind(auction)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Auction not found."))?;
    if let Some(old) = sqlx::query(
        "SELECT id,auction_id,units,max_price_micro FROM market_bids WHERE account_id=$1 AND client_order_id=$2",
    )
    .bind(account.id)
    .bind(client)
    .fetch_optional(&mut *tx)
    .await?
    {
        if old.try_get::<Uuid, _>("auction_id")? != auction
            || old.try_get::<i64, _>("units")? != units
            || old.try_get::<i64, _>("max_price_micro")? != price
        {
            return Err(ApiError::conflict("This order identifier was already used for a different bid."));
        }
        return Ok(Json(json!({"id":old.try_get::<Uuid,_>("id")?,"duplicate":true})));
    }
    if lot.try_get::<String, _>("status")? != "open" || !lot.try_get::<bool, _>("accepting")? {
        return Err(ApiError::conflict("This auction is no longer accepting bids."));
    }
    let notional = money(units, lot.try_get("term_days")?, price)?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM market_bids WHERE auction_id=$1 AND account_id=$2 AND status='open'")
            .bind(auction)
            .bind(account.id)
            .fetch_one(&mut *tx)
            .await?;
    if count >= 20 {
        return Err(ApiError::bad_request(
            "Cancel an existing bid before adding more than 20 open bids in one auction.",
        ));
    }
    let moved = sqlx::query(
        "UPDATE accounts SET balance_micro=balance_micro-$2,held_micro=held_micro+$2 WHERE id=$1 AND balance_micro>=$2",
    )
    .bind(account.id)
    .bind(notional)
    .execute(&mut *tx)
    .await?;
    if moved.rows_affected() == 0 {
        return Err(ApiError::payment_failed("Your available credit does not cover this bid's maximum cost."));
    }
    let hold = Uuid::new_v4();
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO holds(id,account_id,kind,amount_micro,expires_at) VALUES($1,$2,'auction',$3,$4+interval '1 hour')",
    )
    .bind(hold)
    .bind(account.id)
    .bind(notional)
    .bind(lot.try_get::<chrono::DateTime<chrono::Utc>, _>("ends_at")?)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO market_bids(id,auction_id,account_id,client_order_id,units,max_price_micro,hold_id) VALUES($1,$2,$3,$4,$5,$6,$7)").bind(id).bind(auction).bind(account.id).bind(client).bind(units).bind(price).bind(hold).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"held_micro_usd":notional,"status":"open"})))
}

pub async fn cancel_bid(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = control::user(&state, &headers)?;
    let account = control::owned_account(&state, &user).await?;
    let mut tx = state.db.begin().await?;
    let auction: Uuid = sqlx::query_scalar("SELECT auction_id FROM market_bids WHERE id=$1 AND account_id=$2")
        .bind(id)
        .bind(account.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::not_found("Bid not found."))?;
    let lot = sqlx::query("SELECT status,ends_at>now() AS accepting FROM market_auctions WHERE id=$1 FOR UPDATE")
        .bind(auction)
        .fetch_one(&mut *tx)
        .await?;
    let bid = sqlx::query("SELECT status,hold_id FROM market_bids WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if bid.try_get::<String, _>("status")? == "cancelled" {
        return Ok(Json(json!({"cancelled":true})));
    }
    if lot.try_get::<String, _>("status")? != "open" || !lot.try_get::<bool, _>("accepting")? {
        return Err(ApiError::conflict("Bids cannot be cancelled after the auction closes."));
    }
    sqlx::query("UPDATE market_bids SET status='cancelled' WHERE id=$1").bind(id).execute(&mut *tx).await?;
    ledger::release_hold(&mut tx, bid.try_get("hold_id")?).await?;
    tx.commit().await?;
    Ok(Json(json!({"cancelled":true})))
}

pub async fn clear_due(state: &Shared) -> anyhow::Result<u64> {
    let mut count = 0;
    loop {
        let mut tx = state.db.begin().await?;
        let Some(lot)=sqlx::query("SELECT * FROM market_auctions WHERE status='open' AND ends_at<=now() ORDER BY ends_at FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await? else{return Ok(count)};
        let id: Uuid = lot.try_get("id")?;
        let reserve: i64 = lot.try_get("reserve_price_micro")?;
        let capacity: i64 = lot.try_get("units")?;
        let bids=sqlx::query("SELECT * FROM market_bids WHERE auction_id=$1 AND status='open' ORDER BY max_price_micro DESC,created_at,id FOR UPDATE").bind(id).fetch_all(&mut *tx).await?;
        let mut remaining = capacity;
        let mut fills = Vec::new();
        let mut demand: i128 = 0;
        let mut marginal = reserve;
        for bid in &bids {
            let eligible = bid.try_get::<i64, _>("max_price_micro")? >= reserve;
            if eligible {
                demand += bid.try_get::<i64, _>("units")? as i128;
            }
            let fill = if eligible { remaining.min(bid.try_get::<i64, _>("units")?) } else { 0 };
            remaining -= fill;
            if fill > 0 {
                marginal = bid.try_get("max_price_micro")?;
            }
            fills.push(fill);
        }
        let price = if demand > capacity as i128 { marginal } else { reserve };
        for (bid, fill) in bids.iter().zip(fills) {
            let bid_id: Uuid = bid.try_get("id")?;
            let hold: Uuid = bid.try_get("hold_id")?;
            let charged = money(fill, lot.try_get("term_days")?, price).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            if charged > 0 {
                let actual =
                    ledger::charge_from_hold(&mut tx, hold, charged, &format!("auction:{id}:bid:{bid_id}")).await?;
                anyhow::ensure!(actual == charged, "Auction bid hold does not cover its fill");
                sqlx::query("INSERT INTO capacity_contracts(id,auction_id,bid_id,account_id,route_id,units,tokens_per_iu,prompt_weight_bps,ends_at,paid_micro) VALUES($1,$2,$3,$4,$5,$6,$7,$8,now()+make_interval(days=>$9),$10)")
                    .bind(Uuid::new_v4()).bind(id).bind(bid_id).bind(bid.try_get::<Uuid,_>("account_id")?).bind(lot.try_get::<String,_>("route_id")?).bind(fill).bind(lot.try_get::<i64,_>("tokens_per_iu")?).bind(lot.try_get::<i32,_>("prompt_weight_bps")?).bind(lot.try_get::<i32,_>("term_days")?).bind(charged).execute(&mut *tx).await?;
            }
            let status = if fill == 0 {
                "outbid"
            } else if fill < bid.try_get::<i64, _>("units")? {
                "partial"
            } else {
                "filled"
            };
            sqlx::query("UPDATE market_bids SET status=$2,filled_units=$3,charged_micro=$4 WHERE id=$1")
                .bind(bid_id)
                .bind(status)
                .bind(fill)
                .bind(charged)
                .execute(&mut *tx)
                .await?;
            ledger::release_hold(&mut tx, hold).await?;
        }
        sqlx::query("UPDATE market_auctions SET status='cleared',cleared_at=now(),clearing_price_micro=$2,filled_units=$3 WHERE id=$1").bind(id).bind(if capacity==remaining { None } else { Some(price) }).bind(capacity-remaining).execute(&mut *tx).await?;
        tx.commit().await?;
        count += 1;
    }
}

pub async fn contracts(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let user = control::user(&state, &headers)?;
    let mut connection = state.db.acquire().await?;
    let Some(account) = control::account_id(&mut connection, &user, false).await? else {
        return Ok(Json(json!({"contracts": []})));
    };
    drop(connection);
    contracts_for_account(&state, account).await
}

pub async fn api_contracts(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let account = crate::billing::require_account(&state, &headers).await?;
    contracts_for_account(&state, account.id).await
}

async fn contracts_for_account(state: &Shared, account: Uuid) -> Result<Json<Value>, ApiError> {
    let rows=sqlx::query("SELECT c.*,c.starts_at<=now() AND c.ends_at>now() AS active,COALESCE(u.used_bps,0) AS used_bps,COALESCE(u.held_bps,0) AS held_bps FROM capacity_contracts c LEFT JOIN capacity_daily_usage u ON u.contract_id=c.id AND u.day_index=floor(extract(epoch FROM(now()-c.starts_at))/86400)::int WHERE c.account_id=$1 ORDER BY c.starts_at DESC LIMIT 100").bind(account).fetch_all(&state.db).await?;
    let data=rows.into_iter().map(|r|{
        let quota=r.try_get::<i64,_>("units")? as f64*r.try_get::<i64,_>("tokens_per_iu")? as f64;
        let used=r.try_get::<i64,_>("used_bps")? as f64/10_000.0;let held=r.try_get::<i64,_>("held_bps")? as f64/10_000.0;
        Ok(json!({"id":r.try_get::<Uuid,_>("id")?,"auction_id":r.try_get::<Uuid,_>("auction_id")?,"route_id":r.try_get::<String,_>("route_id")?,"units":r.try_get::<i64,_>("units")?,"starts_at":r.try_get::<chrono::DateTime<chrono::Utc>,_>("starts_at")?,"ends_at":r.try_get::<chrono::DateTime<chrono::Utc>,_>("ends_at")?,"active":r.try_get::<bool,_>("active")?,"paid_micro_usd":r.try_get::<i64,_>("paid_micro")?,"daily_tokens":quota,"used_tokens":used,"held_tokens":held,"remaining_tokens":(quota-used-held).max(0.0)}))
    }).collect::<Result<Vec<_>,sqlx::Error>>()?;
    Ok(Json(json!({"contracts":data})))
}

pub async fn contract_route(state: &Shared, account: Uuid, id: Uuid) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT route_id FROM capacity_contracts WHERE id=$1 AND account_id=$2 AND starts_at<=now() AND ends_at>now()",
    )
    .bind(id)
    .bind(account)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Active capacity contract not found."))
}

pub async fn reserve_capacity(
    state: &Shared,
    account: Uuid,
    id: Uuid,
    prompt: u64,
    completion: u64,
) -> Result<Uuid, ApiError> {
    let mut tx = state.db.begin().await?;
    let contract=sqlx::query("SELECT *,floor(extract(epoch FROM(now()-starts_at))/86400)::int AS day FROM capacity_contracts WHERE id=$1 AND account_id=$2 AND starts_at<=now() AND ends_at>now() FOR UPDATE").bind(id).bind(account).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("Active capacity contract not found."))?;
    let amount = i64::try_from(
        prompt as u128 * contract.try_get::<i32, _>("prompt_weight_bps")? as u128 + completion as u128 * 10_000,
    )
    .map_err(|_| ApiError::bad_request("Request token budget is too large."))?;
    let limit =
        contract.try_get::<i64, _>("units")? as i128 * contract.try_get::<i64, _>("tokens_per_iu")? as i128 * 10_000;
    let limit = i64::try_from(limit).map_err(|_| ApiError::bad_request("Contract capacity is too large."))?;
    let day: i32 = contract.try_get("day")?;
    sqlx::query("INSERT INTO capacity_daily_usage(contract_id,day_index) VALUES($1,$2) ON CONFLICT DO NOTHING")
        .bind(id)
        .bind(day)
        .execute(&mut *tx)
        .await?;
    let moved=sqlx::query("UPDATE capacity_daily_usage SET held_bps=held_bps+$3 WHERE contract_id=$1 AND day_index=$2 AND used_bps+held_bps<=$4-$3").bind(id).bind(day).bind(amount).bind(limit).execute(&mut *tx).await?;
    if moved.rows_affected() == 0 {
        return Err(ApiError::payment_failed(
            "This request exceeds the contract's remaining daily token capacity. Lower max_tokens or use prepaid credit.",
        ));
    }
    let hold = Uuid::new_v4();
    sqlx::query("INSERT INTO capacity_holds(id,contract_id,day_index,amount_bps) VALUES($1,$2,$3,$4)")
        .bind(hold)
        .bind(id)
        .bind(day)
        .bind(amount)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(hold)
}

pub async fn finalize_capacity(
    state: &crate::AppState,
    account: Uuid,
    hold: Uuid,
    request: &str,
    charge: Option<&Charge<'_>>,
) -> sqlx::Result<Receipt> {
    for attempt in 0..4 {
        match try_finalize_capacity(state, account, hold, request, charge).await {
            Ok(receipt) => return Ok(receipt),
            Err(error) if attempt == 3 => return Err(error),
            Err(error) => {
                tracing::warn!(%error, request_id=request, "retrying capacity usage finalization");
                tokio::time::sleep(std::time::Duration::from_millis(100 * (attempt + 1))).await;
            }
        }
    }
    unreachable!()
}

async fn try_finalize_capacity(
    state: &crate::AppState,
    account: Uuid,
    hold: Uuid,
    request: &str,
    charge: Option<&Charge<'_>>,
) -> sqlx::Result<Receipt> {
    let mut tx = state.db.begin().await?;
    let row=sqlx::query("SELECT h.*,c.prompt_weight_bps FROM capacity_holds h JOIN capacity_contracts c ON c.id=h.contract_id WHERE h.id=$1 AND h.status='active' FOR UPDATE OF h").bind(hold).fetch_optional(&mut *tx).await?;
    if let Some(row) = row {
        let reserved: i64 = row.try_get("amount_bps")?;
        let contract: Uuid = row.try_get("contract_id")?;
        let used = charge
            .map(|c| {
                (c.usage.prompt_tokens as u128 * row.get::<i32, _>("prompt_weight_bps") as u128
                    + c.usage.completion_tokens as u128 * 10_000)
                    .min(reserved as u128) as i64
            })
            .unwrap_or(0);
        sqlx::query("UPDATE capacity_daily_usage SET held_bps=held_bps-$3,used_bps=used_bps+$4 WHERE contract_id=$1 AND day_index=$2").bind(contract).bind(row.try_get::<i32,_>("day_index")?).bind(reserved).bind(used).execute(&mut *tx).await?;
        sqlx::query("UPDATE capacity_holds SET status='closed' WHERE id=$1").bind(hold).execute(&mut *tx).await?;
        if let Some(c) = charge {
            ledger::record_usage(
                &mut tx,
                &ledger::UsageRecord {
                    capacity_contract_id: Some(contract),
                    request_id: request,
                    account_id: Some(account),
                    payment_id: None,
                    route_id: c.route_id,
                    tier: c.tier.as_str(),
                    prompt_tokens: c.usage.prompt_tokens,
                    completion_tokens: c.usage.completion_tokens,
                    estimated: c.usage.estimated,
                    cost_micro: 0,
                },
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(Receipt::default())
}

pub async fn release_expired_capacity(state: &Shared) -> sqlx::Result<()> {
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM capacity_holds WHERE status='active' AND expires_at<now() LIMIT 100")
            .fetch_all(&state.db)
            .await?;
    for id in ids {
        finalize_capacity(state, Uuid::nil(), id, "expired", None).await?;
    }
    Ok(())
}
