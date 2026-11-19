//! Reference for completing mongo-shell statements (MongoDB, and CouchDB's
//! Mango queries written the same way): methods, operators and pipeline
//! stages, each with a one-line description from the MongoDB and CouchDB
//! documentation.

/// `(name, signature, description)`
pub type Entry = (&'static str, &'static str, &'static str);

/// `db.<method>(…)`
pub const DB_METHODS: &[Entry] = &[
    ("getCollection", "getCollection(\"name\")", "A collection by name (for names with dots or dashes)"),
    ("getCollectionNames", "getCollectionNames()", "Names of the collections in this database"),
    ("createCollection", "createCollection(\"name\")", "Create a collection (a database on CouchDB)"),
    ("runCommand", "runCommand({command})", "Run a database command, e.g. {ping: 1} or {dbStats: 1}"),
    ("stats", "stats()", "Storage statistics for the database"),
    ("dropDatabase", "dropDatabase()", "Delete the current database and its data"),
];

/// `db.<collection>.<method>(…)`
pub const COLLECTION_METHODS: &[Entry] = &[
    ("find", "find(filter, projection)", "Documents matching the filter; chain .sort() .limit() .skip()"),
    ("findOne", "findOne(filter, projection)", "The first document matching the filter"),
    ("aggregate", "aggregate([stages])", "Run an aggregation pipeline ($match, $group, $sort, …)"),
    ("countDocuments", "countDocuments(filter)", "Number of documents matching the filter"),
    ("estimatedDocumentCount", "estimatedDocumentCount()", "Fast document count from collection metadata"),
    ("distinct", "distinct(\"field\", filter)", "Distinct values of a field"),
    ("insertOne", "insertOne({document})", "Insert one document"),
    ("insertMany", "insertMany([documents])", "Insert several documents"),
    ("updateOne", "updateOne(filter, {$set: {…}})", "Update the first matching document"),
    ("updateMany", "updateMany(filter, {$set: {…}})", "Update every matching document"),
    ("replaceOne", "replaceOne(filter, {document})", "Replace the first matching document"),
    ("deleteOne", "deleteOne(filter)", "Delete the first matching document"),
    ("deleteMany", "deleteMany(filter)", "Delete every matching document ({} deletes all)"),
    ("createIndex", "createIndex({field: 1})", "Create an index (1 ascending, -1 descending)"),
    ("getIndexes", "getIndexes()", "The collection's indexes"),
    ("drop", "drop()", "Delete the collection (the database on CouchDB) and its data"),
];

/// Methods chained after `find(…)`.
pub const CURSOR_METHODS: &[Entry] = &[
    ("sort", "sort({field: 1})", "Order results (1 ascending, -1 descending)"),
    ("limit", "limit(n)", "Return at most n documents"),
    ("skip", "skip(n)", "Skip the first n documents"),
    ("projection", "projection({field: 1})", "Choose which fields to return"),
    ("pretty", "pretty()", "Accepted for compatibility; results are always shown in full"),
];

/// Query (filter) operators. Those marked CouchDB are Mango-only.
pub const QUERY_OPERATORS: &[Entry] = &[
    ("$eq", "{field: {$eq: value}}", "Equal to a value"),
    ("$ne", "{field: {$ne: value}}", "Not equal to a value"),
    ("$gt", "{field: {$gt: value}}", "Greater than"),
    ("$gte", "{field: {$gte: value}}", "Greater than or equal to"),
    ("$lt", "{field: {$lt: value}}", "Less than"),
    ("$lte", "{field: {$lte: value}}", "Less than or equal to"),
    ("$in", "{field: {$in: [a, b]}}", "Equal to any value in the array"),
    ("$nin", "{field: {$nin: [a, b]}}", "Equal to none of the values in the array"),
    ("$and", "{$and: [{…}, {…}]}", "Every condition matches"),
    ("$or", "{$or: [{…}, {…}]}", "At least one condition matches"),
    ("$nor", "{$nor: [{…}, {…}]}", "No condition matches"),
    ("$not", "{field: {$not: {…}}}", "The condition doesn't match"),
    ("$exists", "{field: {$exists: true}}", "The field is present (or absent with false)"),
    ("$type", "{field: {$type: \"string\"}}", "The field has this BSON / JSON type"),
    ("$regex", "{field: {$regex: \"^a\"}}", "Matches a regular expression"),
    ("$options", "{$regex: \"x\", $options: \"i\"}", "Regex flags: i (ignore case), m, x, s"),
    ("$size", "{field: {$size: n}}", "The array has exactly n elements"),
    ("$all", "{field: {$all: [a, b]}}", "The array contains every value"),
    ("$elemMatch", "{field: {$elemMatch: {…}}}", "An array element matches every condition"),
    ("$mod", "{field: {$mod: [divisor, remainder]}}", "Divided by divisor leaves remainder"),
    ("$expr", "{$expr: {$gt: [\"$a\", \"$b\"]}}", "Use aggregation expressions in a filter (MongoDB)"),
    ("$text", "{$text: {$search: \"words\"}}", "Text search on a text index (MongoDB)"),
    ("$allMatch", "{field: {$allMatch: {…}}}", "Every array element matches (CouchDB)"),
    ("$keyMapMatch", "{field: {$keyMapMatch: {…}}}", "A key of the map matches (CouchDB)"),
    ("$beginsWith", "{field: {$beginsWith: \"pre\"}}", "The string starts with a prefix (CouchDB)"),
];

/// Update operators (second argument of updateOne / updateMany).
pub const UPDATE_OPERATORS: &[Entry] = &[
    ("$set", "{$set: {field: value}}", "Set fields"),
    ("$unset", "{$unset: {field: \"\"}}", "Remove fields"),
    ("$inc", "{$inc: {field: 1}}", "Add to a number"),
    ("$mul", "{$mul: {field: 2}}", "Multiply a number"),
    ("$min", "{$min: {field: value}}", "Set if the value is lower"),
    ("$max", "{$max: {field: value}}", "Set if the value is higher"),
    ("$rename", "{$rename: {old: \"new\"}}", "Rename a field"),
    ("$currentDate", "{$currentDate: {field: true}}", "Set a field to the current date"),
    ("$setOnInsert", "{$setOnInsert: {field: value}}", "Set fields only when an upsert inserts"),
    ("$push", "{$push: {array: value}}", "Append to an array"),
    ("$addToSet", "{$addToSet: {array: value}}", "Append to an array unless already present"),
    ("$pull", "{$pull: {array: condition}}", "Remove matching array elements"),
    ("$pullAll", "{$pullAll: {array: [a, b]}}", "Remove every listed value from an array"),
    ("$pop", "{$pop: {array: 1}}", "Remove the last (1) or first (-1) array element"),
    ("$each", "{$push: {array: {$each: [a, b]}}}", "Add several values with $push / $addToSet"),
];

/// Aggregation pipeline stages.
pub const STAGES: &[Entry] = &[
    ("$match", "{$match: {filter}}", "Keep documents matching a filter"),
    ("$group", "{$group: {_id: \"$field\", n: {$sum: 1}}}", "Group documents and compute accumulators"),
    ("$project", "{$project: {field: 1}}", "Choose, rename or compute fields"),
    ("$addFields", "{$addFields: {field: expr}}", "Add computed fields"),
    ("$set", "{$set: {field: expr}}", "Add computed fields (alias of $addFields)"),
    ("$unset", "{$unset: \"field\"}", "Remove fields"),
    ("$sort", "{$sort: {field: -1}}", "Order documents"),
    ("$limit", "{$limit: n}", "Keep the first n documents"),
    ("$skip", "{$skip: n}", "Skip the first n documents"),
    ("$count", "{$count: \"name\"}", "Count documents into one field"),
    ("$unwind", "{$unwind: \"$array\"}", "One document per array element"),
    ("$lookup", "{$lookup: {from, localField, foreignField, as}}", "Join documents from another collection"),
    ("$facet", "{$facet: {name: [stages]}}", "Run several pipelines on the same input"),
    ("$bucket", "{$bucket: {groupBy, boundaries}}", "Group into ranges"),
    ("$sortByCount", "{$sortByCount: \"$field\"}", "Group by a value and sort by count"),
    ("$replaceRoot", "{$replaceRoot: {newRoot: \"$doc\"}}", "Replace each document with an embedded one"),
    ("$sample", "{$sample: {size: n}}", "n random documents"),
    ("$out", "{$out: \"collection\"}", "Write the results to a collection (last stage)"),
    ("$merge", "{$merge: {into: \"collection\"}}", "Merge the results into a collection (last stage)"),
];

/// Accumulators and expression operators used inside stages.
pub const EXPRESSIONS: &[Entry] = &[
    ("$sum", "{$sum: \"$field\"}", "Sum (or {$sum: 1} to count)"),
    ("$avg", "{$avg: \"$field\"}", "Average"),
    ("$min", "{$min: \"$field\"}", "Lowest value"),
    ("$max", "{$max: \"$field\"}", "Highest value"),
    ("$first", "{$first: \"$field\"}", "Value from the first document of the group"),
    ("$last", "{$last: \"$field\"}", "Value from the last document of the group"),
    ("$push", "{$push: \"$field\"}", "Array of every value in the group"),
    ("$addToSet", "{$addToSet: \"$field\"}", "Array of distinct values in the group"),
    ("$count", "{$count: {}}", "Number of documents in the group"),
    ("$cond", "{$cond: [if, then, else]}", "If / then / else"),
    ("$ifNull", "{$ifNull: [\"$field\", default]}", "A default for null or missing values"),
    ("$concat", "{$concat: [\"$a\", \" \", \"$b\"]}", "Join strings"),
    ("$toUpper", "{$toUpper: \"$field\"}", "Upper-case a string"),
    ("$toLower", "{$toLower: \"$field\"}", "Lower-case a string"),
    ("$substr", "{$substr: [\"$field\", start, length]}", "Part of a string"),
    ("$add", "{$add: [\"$a\", \"$b\"]}", "Add numbers (or a number to a date)"),
    ("$subtract", "{$subtract: [\"$a\", \"$b\"]}", "Subtract"),
    ("$multiply", "{$multiply: [\"$a\", \"$b\"]}", "Multiply"),
    ("$divide", "{$divide: [\"$a\", \"$b\"]}", "Divide"),
    ("$round", "{$round: [\"$field\", places]}", "Round a number"),
    ("$size", "{$size: \"$array\"}", "Number of array elements"),
    ("$dateToString", "{$dateToString: {format: \"%Y-%m-%d\", date: \"$d\"}}", "Format a date"),
    ("$year", "{$year: \"$date\"}", "Year of a date"),
    ("$month", "{$month: \"$date\"}", "Month of a date"),
    ("$toString", "{$toString: \"$field\"}", "Convert to a string"),
    ("$toInt", "{$toInt: \"$field\"}", "Convert to an integer"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_well_formed() {
        for table in [DB_METHODS, COLLECTION_METHODS, CURSOR_METHODS] {
            for (name, sig, doc) in table {
                assert!(sig.starts_with(name) && sig.contains('('), "{name}");
                assert!(!doc.is_empty());
            }
        }
        for table in [QUERY_OPERATORS, UPDATE_OPERATORS, STAGES, EXPRESSIONS] {
            for (name, sig, doc) in table {
                assert!(name.starts_with('$') && sig.contains(name), "{name}");
                assert!(!doc.is_empty());
            }
        }
    }
}
