use serde_json::{json, Value};
use sqlx::PgPool;

use fluxfang_core::rule::Rule;
use fluxfang_db::models::NewEmitter;
use fluxfang_db::{EmissionRepo, EmitterRepo};

use crate::mcp::tools::ToolError;
use crate::mcp::{audit, shape};

fn parse_rule(args: &Value, key: &str) -> Result<Option<Rule>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value::<Rule>(v.clone())
            .map(Some)
            .map_err(|e| ToolError::InvalidParams(format!("invalid {key}: {e}"))),
    }
}

fn kind_of(args: &Value) -> Result<String, ToolError> {
    shape::opt_str(args, "kind")
        .ok_or_else(|| ToolError::InvalidParams("missing 'kind' (wifi/bluetooth/tpms)".into()))
}

pub async fn create_emitter_from_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let name = shape::opt_str(&args, "name")
        .ok_or_else(|| ToolError::InvalidParams("missing 'name'".into()))?;
    let attributes = args.get("attributes").cloned().unwrap_or_else(|| json!({}));
    let match_criteria = args.get("match_rule").cloned().unwrap_or_else(|| json!({}));
    let rule = parse_rule(&args, "match_rule")?;
    let kind = if rule.is_some() { kind_of(&args)? } else { String::new() };
    let emission_ids = match args.get("emission_ids") {
        Some(_) => shape::parse_uuid_list(&args, "emission_ids")?,
        None => Vec::new(),
    };

    let emitter = EmitterRepo::insert(pool, NewEmitter {
        name: name.clone(),
        type_: shape::opt_str(&args, "type"),
        emitter_type: shape::opt_str(&args, "emitter_type"),
        attributes,
        match_criteria: match_criteria.clone(),
        source: "ai".to_string(),
        ..Default::default()
    }).await?;

    // Attach explicitly-listed emissions.
    let mut affected = vec![emitter.id];
    for eid in &emission_ids {
        EmissionRepo::set_emitter(pool, *eid, emitter.id).await?;
        affected.push(*eid);
    }

    // If a match rule was given, persist it and retroactively claim matches.
    let mut claimed = emission_ids.len() as u64;
    if let Some(rule) = &rule {
        EmitterRepo::update_rule(pool, emitter.id, &match_criteria).await?;
        claimed += EmitterRepo::attach_emissions_matching(pool, emitter.id, rule, &kind).await
            .map_err(|e| ToolError::Db(format!("{e:?}")))?;
    }

    let emitter = EmitterRepo::get(pool, emitter.id).await?
        .ok_or_else(|| ToolError::NotFound("emitter vanished".into()))?;
    let result = json!({ "emitter": shape::emitter_json(&emitter), "emissions_claimed": claimed });
    audit::record_success(
        pool, "create_emitter_from_emissions", "add",
        format!("Created emitter '{name}' ({claimed} emission(s) claimed)"),
        &args, &result, affected,
    ).await;
    Ok(result)
}

pub async fn set_emitter_match_rule(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "emitter_id")?;
    let match_criteria = args.get("match_rule").cloned()
        .ok_or_else(|| ToolError::InvalidParams("missing 'match_rule'".into()))?;
    let rule = parse_rule(&args, "match_rule")?
        .ok_or_else(|| ToolError::InvalidParams("'match_rule' must be a rule object".into()))?;
    let kind = kind_of(&args)?;

    let emitter = EmitterRepo::update_rule(pool, id, &match_criteria).await?;
    let claimed = EmitterRepo::attach_emissions_matching(pool, id, &rule, &kind).await
        .map_err(|e| ToolError::Db(format!("{e:?}")))?;

    let result = json!({ "emitter": shape::emitter_json(&emitter), "emissions_claimed": claimed });
    audit::record_success(
        pool, "set_emitter_match_rule", "add",
        format!("Set match rule on emitter {id} ({claimed} claimed)"),
        &args, &result, vec![id],
    ).await;
    Ok(result)
}

pub async fn preview_match_rule(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    // Read-only: no audit row.
    let rule = parse_rule(&args, "match_rule")?
        .ok_or_else(|| ToolError::InvalidParams("missing 'match_rule'".into()))?;
    let kind = kind_of(&args)?;
    let n = EmitterRepo::count_matching(pool, &rule, &kind).await
        .map_err(|e| ToolError::Db(format!("{e:?}")))?;
    Ok(json!({ "would_match": n }))
}

pub async fn attach_emissions(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let emitter_id = shape::parse_uuid(&args, "emitter_id")?;
    let ids = shape::parse_uuid_list(&args, "emission_ids")?;
    for eid in &ids {
        EmissionRepo::set_emitter(pool, *eid, emitter_id).await?;
    }
    let mut affected = vec![emitter_id];
    affected.extend(ids.iter().copied());
    let result = json!({ "emitter_id": emitter_id, "attached": ids.len() });
    audit::record_success(
        pool, "attach_emissions", "add",
        format!("Attached {} emission(s) to emitter {emitter_id}", ids.len()),
        &args, &result, affected,
    ).await;
    Ok(result)
}

pub async fn update_emitter(pool: &PgPool, args: Value) -> Result<Value, ToolError> {
    let id = shape::parse_uuid(&args, "emitter_id")?;
    let existing = EmitterRepo::get(pool, id).await?
        .ok_or_else(|| ToolError::NotFound(format!("emitter {id}")))?;

    // Rename / retype if provided.
    let name = shape::opt_str(&args, "name").unwrap_or(existing.name.clone());
    let type_ = shape::opt_str(&args, "type").or(existing.type_.clone());
    let emitter = EmitterRepo::update_basic(pool, id, &name, type_.as_deref()).await?;

    // Merge attributes if provided (shallow merge into existing attributes).
    let emitter = if let Some(patch) = args.get("attributes") {
        let mut merged = emitter.attributes.clone();
        if let (Some(m), Some(p)) = (merged.as_object_mut(), patch.as_object()) {
            for (k, v) in p { m.insert(k.clone(), v.clone()); }
        }
        EmitterRepo::set_attributes(pool, id, &merged).await?
    } else {
        emitter
    };

    let result = json!({ "emitter": shape::emitter_json(&emitter) });
    audit::record_success(
        pool, "update_emitter", "add",
        format!("Updated emitter {id} ('{name}')"),
        &args, &result, vec![id],
    ).await;
    Ok(result)
}
