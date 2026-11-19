//! MongoDB-style update operators applied to CouchDB documents. CouchDB has
//! no partial updates: DataBug reads the matching documents (with their
//! `_rev`), applies the operators here, and writes the whole documents back.

use serde_json::{Map, Value};

/// Apply an update document (`{$set: {...}, $inc: {...}}`) to `doc`.
/// Fields may be dotted paths (`address.city`). `_id` and `_rev` can't change.
pub(crate) fn apply(doc: &mut Map<String, Value>, update: &Value) -> Result<(), String> {
    let ops = update.as_object().ok_or("The update must be a document like {$set: {status: \"paid\"}}")?;
    if ops.is_empty() {
        return Err("The update is empty".into());
    }
    if let Some(k) = ops.keys().find(|k| !k.starts_with('$')) {
        return Err(format!("`{k}` isn't an update operator. Use {{$set: {{{k}: …}}}}, or replaceOne() to replace the whole document"));
    }
    for (op, fields) in ops {
        let fields = fields.as_object().ok_or_else(|| format!("{op} takes a document of fields"))?;
        for (path, v) in fields {
            if path == "_id" || path == "_rev" || path.starts_with("_id.") {
                return Err(format!("{op} can't change {path}"));
            }
            match op.as_str() {
                "$set" => set(doc, path, v.clone()),
                "$unset" => {
                    remove(doc, path);
                }
                "$inc" | "$mul" => {
                    let by = v.as_f64().ok_or_else(|| format!("{op} needs a number for {path}"))?;
                    let cur = get(doc, path).cloned().unwrap_or(Value::from(0));
                    let n = cur.as_f64().ok_or_else(|| format!("{path} isn't a number, so {op} can't change it"))?;
                    let r = if op == "$inc" { n + by } else { n * by };
                    let both_ints = cur.is_i64() && v.is_i64() || (get(doc, path).is_none() && v.is_i64());
                    set(doc, path, if both_ints && r.fract() == 0.0 && r.abs() < 9.0e15 { Value::from(r as i64) } else { Value::from(r) });
                }
                "$min" | "$max" => {
                    let replace = match get(doc, path) {
                        None => true,
                        Some(cur) => match (cur.as_f64(), v.as_f64(), cur.as_str(), v.as_str()) {
                            (Some(a), Some(b), _, _) => (op == "$min" && b < a) || (op == "$max" && b > a),
                            (_, _, Some(a), Some(b)) => (op == "$min" && b < a) || (op == "$max" && b > a),
                            _ => false,
                        },
                    };
                    if replace {
                        set(doc, path, v.clone());
                    }
                }
                "$rename" => {
                    let to = v.as_str().ok_or_else(|| format!("$rename needs the new name of {path} as a string"))?;
                    if let Some(val) = remove(doc, path) {
                        set(doc, to, val);
                    }
                }
                "$currentDate" => set(doc, path, Value::from(now_rfc3339())),
                "$setOnInsert" => {}
                "$push" | "$addToSet" => {
                    let items: Vec<Value> = match v.as_object().and_then(|o| o.get("$each")) {
                        Some(Value::Array(a)) => a.clone(),
                        Some(_) => return Err(format!("$each in {op} needs an array")),
                        None => vec![v.clone()],
                    };
                    let arr = array_at(doc, path, op)?;
                    for it in items {
                        if op == "$push" || !arr.contains(&it) {
                            arr.push(it);
                        }
                    }
                }
                "$pull" | "$pullAll" => {
                    let gone: Vec<Value> = match (op.as_str(), v) {
                        ("$pullAll", Value::Array(a)) => a.clone(),
                        ("$pullAll", _) => return Err("$pullAll needs an array of values".into()),
                        (_, Value::Object(o)) if o.keys().any(|k| k.starts_with('$')) => return Err("$pull with a condition isn't supported for CouchDB; give the value to remove".into()),
                        (_, other) => vec![other.clone()],
                    };
                    if get(doc, path).is_some() {
                        array_at(doc, path, op)?.retain(|x| !gone.contains(x));
                    }
                }
                "$pop" => {
                    let first = v.as_i64() == Some(-1);
                    if get(doc, path).is_some() {
                        let arr = array_at(doc, path, op)?;
                        if !arr.is_empty() {
                            if first {
                                arr.remove(0);
                            } else {
                                arr.pop();
                            }
                        }
                    }
                }
                other => return Err(format!("{other} isn't supported for CouchDB. Supported: $set, $unset, $inc, $mul, $min, $max, $rename, $currentDate, $push, $addToSet, $pull, $pullAll, $pop")),
            }
        }
    }
    Ok(())
}

fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil-from-days (Howard Hinnant), for UTC
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn get<'a>(doc: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut cur = doc.get(parts.next()?)?;
    for p in parts {
        cur = match cur {
            Value::Object(o) => o.get(p)?,
            Value::Array(a) => a.get(p.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

/// Set a dotted path, creating objects along the way.
fn set(doc: &mut Map<String, Value>, path: &str, v: Value) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = doc;
    for p in &parts[..parts.len() - 1] {
        let entry = cur.entry(p.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        cur = entry.as_object_mut().unwrap();
    }
    cur.insert(parts[parts.len() - 1].to_string(), v);
}

fn remove(doc: &mut Map<String, Value>, path: &str) -> Option<Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = doc;
    for p in &parts[..parts.len() - 1] {
        cur = cur.get_mut(*p)?.as_object_mut()?;
    }
    cur.remove(parts[parts.len() - 1])
}

fn array_at<'a>(doc: &'a mut Map<String, Value>, path: &str, op: &str) -> Result<&'a mut Vec<Value>, String> {
    if get(doc, path).is_none() {
        set(doc, path, Value::Array(Vec::new()));
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut cur = doc;
    for p in &parts[..parts.len() - 1] {
        cur = cur.get_mut(*p).and_then(|v| v.as_object_mut()).ok_or_else(|| format!("{path} isn't reachable"))?;
    }
    cur.get_mut(parts[parts.len() - 1]).and_then(|v| v.as_array_mut()).ok_or_else(|| format!("{path} isn't an array, so {op} can't change it"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(doc: Value, update: Value) -> Result<Value, String> {
        let mut d = doc.as_object().unwrap().clone();
        apply(&mut d, &update)?;
        Ok(Value::Object(d))
    }

    #[test]
    fn set_unset_inc_and_paths() {
        let d = run(json!({"_id": "a", "n": 1, "x": 1, "tags": ["a"]}), json!({"$set": {"status": "paid", "address.city": "Macon"}, "$unset": {"x": ""}, "$inc": {"n": 2, "views": 1}})).unwrap();
        assert_eq!(d, json!({"_id": "a", "n": 3, "views": 1, "tags": ["a"], "status": "paid", "address": {"city": "Macon"}}));
        let d = run(json!({"price": 10.0}), json!({"$mul": {"price": 1.5}})).unwrap();
        assert_eq!(d["price"], 15.0);
    }

    #[test]
    fn arrays() {
        let d = run(json!({"tags": ["a", "b"]}), json!({"$push": {"tags": "c"}, "$addToSet": {"more": {"$each": ["x", "x", "y"]}}})).unwrap();
        assert_eq!(d["tags"], json!(["a", "b", "c"]));
        assert_eq!(d["more"], json!(["x", "y"]));
        let d = run(json!({"tags": ["a", "b", "a"]}), json!({"$pull": {"tags": "a"}})).unwrap();
        assert_eq!(d["tags"], json!(["b"]));
        let d = run(json!({"q": [1, 2, 3]}), json!({"$pop": {"q": -1}})).unwrap();
        assert_eq!(d["q"], json!([2, 3]));
    }

    #[test]
    fn min_max_rename_and_dates() {
        let d = run(json!({"lo": 5, "hi": 5, "old": 1}), json!({"$min": {"lo": 3}, "$max": {"hi": 3}, "$rename": {"old": "new"}, "$currentDate": {"at": true}})).unwrap();
        assert_eq!((d["lo"].as_i64(), d["hi"].as_i64(), d["new"].as_i64()), (Some(3), Some(5), Some(1)));
        assert!(d.get("old").is_none());
        assert!(d["at"].as_str().unwrap().ends_with('Z') && d["at"].as_str().unwrap().len() == 20);
    }

    #[test]
    fn refuses_what_it_cant_do() {
        assert!(run(json!({}), json!({"status": "paid"})).unwrap_err().contains("replaceOne"));
        assert!(run(json!({}), json!({"$set": {"_rev": "1-x"}})).is_err());
        assert!(run(json!({"n": "x"}), json!({"$inc": {"n": 1}})).is_err());
        assert!(run(json!({}), json!({"$bit": {"n": {"and": 1}}})).unwrap_err().contains("Supported"));
        assert!(run(json!({"t": [1]}), json!({"$pull": {"t": {"$gt": 0}}})).is_err());
    }
}
