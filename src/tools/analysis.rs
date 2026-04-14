use std::fs::File;

use anyhow::{anyhow, bail, Context, Result};
use polars::prelude::*;
use serde_json::{json, Value};

use crate::tools::Sandbox;

pub fn tool_specs() -> Vec<Value> {
    vec![
        df_inspect_spec(),
        df_describe_spec(),
        df_filter_spec(),
        df_group_stats_spec(),
        df_value_counts_spec(),
        df_correlation_spec(),
    ]
}

pub fn execute(sandbox: &Sandbox, name: &str, args: &Value) -> Result<String> {
    match name {
        "df_inspect" => execute_df_inspect(sandbox, args),
        "df_describe" => execute_df_describe(sandbox, args),
        "df_filter" => execute_df_filter(sandbox, args),
        "df_group_stats" => execute_df_group_stats(sandbox, args),
        "df_value_counts" => execute_df_value_counts(sandbox, args),
        "df_correlation" => execute_df_correlation(sandbox, args),
        _ => bail!("unknown analysis tool: {}", name),
    }
}

pub fn tool_slug(name: &str, args: &Value) -> Option<String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
    match name {
        "df_inspect" => Some(format!("Inspecting dataframe {}", path)),
        "df_describe" => Some(format!("Computing dataframe statistics for {}", path)),
        "df_filter" => Some(format!("Filtering dataframe {}", path)),
        "df_group_stats" => Some(format!("Grouping dataframe {}", path)),
        "df_value_counts" => Some(format!("Computing value counts for {}", path)),
        "df_correlation" => Some(format!("Computing column correlation for {}", path)),
        _ => None,
    }
}

fn execute_df_inspect(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let limit = usize_arg(args, "limit").unwrap_or(10).clamp(1, 100);
    let df = read_dataframe(sandbox, path)?;
    let schema = df.schema();
    let mut schema_lines = Vec::new();
    for field in schema.iter_fields() {
        schema_lines.push(format!("- {}: {:?}", field.name(), field.dtype()));
    }
    let preview = df.head(Some(limit));
    Ok(format!(
        "shape: {} rows x {} columns\nschema:\n{}\npreview:\n{}",
        df.height(),
        df.width(),
        schema_lines.join("\n"),
        preview
    ))
}

fn execute_df_describe(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let columns = string_list_arg(args, "columns");
    let df = read_dataframe(sandbox, path)?;
    let target_columns = columns.unwrap_or_else(|| numeric_columns(&df));
    if target_columns.is_empty() {
        bail!("no numeric columns available for describe");
    }

    let expressions: Vec<Expr> = target_columns
        .iter()
        .flat_map(|column| {
            [
                col(column).count().alias(&format!("{}_count", column)),
                col(column).null_count().alias(&format!("{}_nulls", column)),
                col(column).mean().alias(&format!("{}_mean", column)),
                col(column).median().alias(&format!("{}_median", column)),
                col(column).std(1).alias(&format!("{}_std", column)),
                col(column).min().alias(&format!("{}_min", column)),
                col(column).max().alias(&format!("{}_max", column)),
            ]
        })
        .collect();

    let stats = df.lazy().select(expressions).collect()?;
    Ok(format!(
        "describe columns: {}\n{}",
        target_columns.join(", "),
        stats
    ))
}

fn execute_df_filter(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let limit = usize_arg(args, "limit").unwrap_or(25).clamp(1, 200);
    let filters = args
        .get("filters")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing or invalid `filters`"))?;

    let df = read_dataframe(sandbox, path)?;
    let mut lf = df.lazy();
    for filter in filters {
        lf = lf.filter(parse_filter(filter)?);
    }

    if let Some(columns) = string_list_arg(args, "select") {
        let exprs: Vec<Expr> = columns.iter().map(col).collect();
        lf = lf.select(exprs);
    }

    let result = lf.limit(limit as IdxSize).collect()?;
    Ok(format!(
        "filtered preview (up to {} rows):\n{}",
        limit, result
    ))
}

fn execute_df_group_stats(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let group_by = string_list_arg(args, "group_by")
        .ok_or_else(|| anyhow!("missing or invalid `group_by`"))?;
    let metrics = args
        .get("metrics")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("missing or invalid `metrics`"))?;
    let limit = usize_arg(args, "limit").unwrap_or(50).clamp(1, 200);

    if group_by.is_empty() {
        bail!("group_by must include at least one column");
    }

    let df = read_dataframe(sandbox, path)?;
    let group_exprs: Vec<Expr> = group_by.iter().map(col).collect();
    let metric_exprs: Vec<Expr> = metrics.iter().map(parse_metric).collect::<Result<_>>()?;

    let result = df
        .lazy()
        .group_by(group_exprs)
        .agg(metric_exprs)
        .limit(limit as IdxSize)
        .collect()?;

    Ok(format!(
        "grouped by {}\n{}",
        group_by.join(", "),
        result
    ))
}

fn execute_df_value_counts(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let column = string_arg(args, "column")?;
    let limit = usize_arg(args, "limit").unwrap_or(25).clamp(1, 200);
    let df = read_dataframe(sandbox, path)?;
    let result = df
        .lazy()
        .group_by([col(column)])
        .agg([len().alias("count")])
        .limit(limit as IdxSize)
        .collect()?;
    Ok(format!("value counts for {}\n{}", column, result))
}

fn execute_df_correlation(sandbox: &Sandbox, args: &Value) -> Result<String> {
    let path = string_arg(args, "path")?;
    let left = string_arg(args, "left")?;
    let right = string_arg(args, "right")?;
    let df = read_dataframe(sandbox, path)?;
    let pair = df
        .lazy()
        .select([col(left).cast(DataType::Float64), col(right).cast(DataType::Float64)])
        .drop_nulls(None)
        .collect()?;
    let left_series = pair.column(left)?.f64()?;
    let right_series = pair.column(right)?.f64()?;
    if left_series.len() != right_series.len() || left_series.len() < 2 {
        bail!("need at least two non-null paired values to compute correlation");
    }

    let n = left_series.len() as f64;
    let left_mean = left_series.sum().ok_or_else(|| anyhow!("left column has no values"))? / n;
    let right_mean =
        right_series.sum().ok_or_else(|| anyhow!("right column has no values"))? / n;

    let mut numerator = 0.0f64;
    let mut left_ss = 0.0f64;
    let mut right_ss = 0.0f64;
    for (l, r) in left_series.into_no_null_iter().zip(right_series.into_no_null_iter()) {
        let l_delta = l - left_mean;
        let r_delta = r - right_mean;
        numerator += l_delta * r_delta;
        left_ss += l_delta * l_delta;
        right_ss += r_delta * r_delta;
    }

    if left_ss == 0.0 || right_ss == 0.0 {
        bail!("correlation is undefined for a constant column");
    }

    let corr = numerator / (left_ss.sqrt() * right_ss.sqrt());
    Ok(format!(
        "pearson correlation between {} and {}: {:.6} (n={})",
        left,
        right,
        corr,
        left_series.len()
    ))
}

fn read_dataframe(sandbox: &Sandbox, path: &str) -> Result<DataFrame> {
    let abs = sandbox.resolve(path)?;
    let ext = abs
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "csv" => {
            let path_buf = abs.clone();
            CsvReadOptions::default()
                .with_has_header(true)
                .try_into_reader_with_file_path(Some(path_buf))?
                .finish()
                .with_context(|| format!("reading csv {}", path))
        }
        "parquet" => {
            let file = File::open(&abs).with_context(|| format!("opening parquet {}", path))?;
            ParquetReader::new(file)
                .finish()
                .with_context(|| format!("reading parquet {}", path))
        }
        "json" => {
            let file = File::open(&abs).with_context(|| format!("opening json {}", path))?;
            JsonReader::new(file)
                .finish()
                .with_context(|| format!("reading json {}", path))
        }
        other => bail!(
            "unsupported dataframe format `{}` for {} (supported: csv, parquet, json)",
            other,
            path
        ),
    }
}

fn numeric_columns(df: &DataFrame) -> Vec<String> {
    df.schema()
        .iter_fields()
        .filter(|field| field.dtype().is_primitive_numeric())
        .map(|field| field.name().to_string())
        .collect()
}

fn parse_filter(value: &Value) -> Result<Expr> {
    let column = value
        .get("column")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("filter missing `column`"))?;
    let op = value
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("filter missing `op`"))?;

    let expr = match op {
        "eq" => col(column).eq(lit_from_json(value.get("value"))?),
        "ne" => col(column).neq(lit_from_json(value.get("value"))?),
        "gt" => col(column).gt(lit_from_json(value.get("value"))?),
        "gte" => col(column).gt_eq(lit_from_json(value.get("value"))?),
        "lt" => col(column).lt(lit_from_json(value.get("value"))?),
        "lte" => col(column).lt_eq(lit_from_json(value.get("value"))?),
        "is_null" => col(column).is_null(),
        "is_not_null" => col(column).is_not_null(),
        other => bail!("unsupported filter op `{}`", other),
    };

    Ok(expr)
}

fn parse_metric(value: &Value) -> Result<Expr> {
    let column = value
        .get("column")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("metric missing `column`"))?;
    let agg = value
        .get("agg")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("metric missing `agg`"))?;

    let expr = match agg {
        "count" => col(column).count().alias(&format!("{}_count", column)),
        "sum" => col(column).sum().alias(&format!("{}_sum", column)),
        "mean" => col(column).mean().alias(&format!("{}_mean", column)),
        "median" => col(column).median().alias(&format!("{}_median", column)),
        "min" => col(column).min().alias(&format!("{}_min", column)),
        "max" => col(column).max().alias(&format!("{}_max", column)),
        "std" => col(column).std(1).alias(&format!("{}_std", column)),
        other => bail!("unsupported metric agg `{}`", other),
    };

    Ok(expr)
}

fn lit_from_json(value: Option<&Value>) -> Result<Expr> {
    let value = value.ok_or_else(|| anyhow!("missing filter `value`"))?;
    match value {
        Value::String(v) => Ok(lit(v.as_str())),
        Value::Bool(v) => Ok(lit(*v)),
        Value::Number(v) => {
            if let Some(i) = v.as_i64() {
                Ok(lit(i))
            } else if let Some(f) = v.as_f64() {
                Ok(lit(f))
            } else {
                bail!("unsupported numeric literal")
            }
        }
        Value::Null => Ok(lit(NULL)),
        _ => bail!("unsupported literal type"),
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("missing or non-string arg `{}`", key))
}

fn usize_arg(args: &Value, key: &str) -> Option<usize> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .and_then(|v| usize::try_from(v).ok())
}

fn string_list_arg(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| {
        v.as_array().map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect()
        })
    })
}

fn df_inspect_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_inspect",
        "description": "Load a CSV, Parquet, or JSON dataframe from the workspace and return its shape, schema, and a preview.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100}
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn df_describe_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_describe",
        "description": "Compute summary statistics for numeric dataframe columns.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "columns": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }
    })
}

fn df_filter_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_filter",
        "description": "Apply one or more dataframe filters and optionally project a subset of columns.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "filters": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "column": {"type": "string"},
                            "op": {
                                "type": "string",
                                "enum": ["eq", "ne", "gt", "gte", "lt", "lte", "is_null", "is_not_null"]
                            },
                            "value": {}
                        },
                        "required": ["column", "op"],
                        "additionalProperties": false
                    }
                },
                "select": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["path", "filters"],
            "additionalProperties": false
        }
    })
}

fn df_group_stats_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_group_stats",
        "description": "Group a dataframe by one or more columns and compute aggregate metrics.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "group_by": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "metrics": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "column": {"type": "string"},
                            "agg": {"type": "string", "enum": ["count", "sum", "mean", "median", "min", "max", "std"]}
                        },
                        "required": ["column", "agg"],
                        "additionalProperties": false
                    }
                },
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["path", "group_by", "metrics"],
            "additionalProperties": false
        }
    })
}

fn df_value_counts_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_value_counts",
        "description": "Count occurrences of unique values in a dataframe column.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "column": {"type": "string"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200}
            },
            "required": ["path", "column"],
            "additionalProperties": false
        }
    })
}

fn df_correlation_spec() -> Value {
    json!({
        "type": "function",
        "name": "df_correlation",
        "description": "Compute the Pearson correlation between two numeric columns.",
        "parameters": {
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "left": {"type": "string"},
                "right": {"type": "string"}
            },
            "required": ["path", "left", "right"],
            "additionalProperties": false
        }
    })
}
