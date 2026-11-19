//! Redis commands as typed in redis-cli: one command per line, arguments
//! separated by spaces, quoted with `"…"` (with backslash escapes) or `'…'`.
//! Lines starting with `#`, `//` or `--` are comments.

use crate::analyze::{Danger, StatementKind};
use std::ops::Range;

/// What a command does, for read-only connections and confirmations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// Server administration and anything with wide effects.
    Admin,
}

/// A command from the Redis documentation: name, access, arguments, summary.
pub struct Command {
    pub name: &'static str,
    pub access: Access,
    pub args: &'static str,
    pub summary: &'static str,
}

const fn c(name: &'static str, access: Access, args: &'static str, summary: &'static str) -> Command {
    Command { name, access, args, summary }
}

use Access::{Admin as A, Read as R, Write as W};

/// The commands DataBug knows (from redis.io/docs/latest/commands), used for
/// highlighting, completion and read/write checks. Others still run; they
/// count as writes.
pub const COMMANDS: &[Command] = &[
    // keys
    c("DEL", W, "key [key ...]", "Delete one or more keys"),
    c("UNLINK", W, "key [key ...]", "Delete keys asynchronously"),
    c("EXISTS", R, "key [key ...]", "Count how many of the keys exist"),
    c("TYPE", R, "key", "Type of the value stored at key"),
    c("TTL", R, "key", "Seconds until the key expires (-1 none, -2 missing)"),
    c("PTTL", R, "key", "Milliseconds until the key expires"),
    c("EXPIRETIME", R, "key", "Unix time when the key expires"),
    c("EXPIRE", W, "key seconds [NX|XX|GT|LT]", "Set a key's time to live in seconds"),
    c("PEXPIRE", W, "key milliseconds", "Set a key's time to live in milliseconds"),
    c("EXPIREAT", W, "key unix-time-seconds", "Expire the key at a Unix time"),
    c("PERSIST", W, "key", "Remove a key's expiry"),
    c("RENAME", W, "key newkey", "Rename a key"),
    c("RENAMENX", W, "key newkey", "Rename a key only if the new name is free"),
    c("COPY", W, "source destination [DB db] [REPLACE]", "Copy a key's value"),
    c("KEYS", R, "pattern", "Keys matching a pattern (blocks the server; prefer SCAN)"),
    c("SCAN", R, "cursor [MATCH pattern] [COUNT count] [TYPE type]", "Iterate over keys"),
    c("RANDOMKEY", R, "", "A random key"),
    c("TOUCH", W, "key [key ...]", "Update the keys' last access time"),
    c("OBJECT", R, "ENCODING|FREQ|IDLETIME|REFCOUNT key", "Internal details of a key's value"),
    c("DUMP", R, "key", "Serialized value of a key"),
    c("RESTORE", W, "key ttl serialized-value [REPLACE]", "Create a key from a DUMP value"),
    c("SORT", W, "key [BY pattern] [LIMIT offset count] [GET pattern] [ASC|DESC] [ALPHA] [STORE destination]", "Sort a list, set or sorted set"),
    c("SORT_RO", R, "key [BY pattern] [LIMIT offset count] [ASC|DESC] [ALPHA]", "Sort without storing"),
    c("MOVE", W, "key db", "Move a key to another database"),
    // strings
    c("GET", R, "key", "Value of a string key"),
    c("SET", W, "key value [NX|XX] [GET] [EX seconds|PX milliseconds|KEEPTTL]", "Set a string value"),
    c("SETNX", W, "key value", "Set only if the key doesn't exist"),
    c("SETEX", W, "key seconds value", "Set with an expiry in seconds"),
    c("PSETEX", W, "key milliseconds value", "Set with an expiry in milliseconds"),
    c("GETSET", W, "key value", "Set a value and return the old one"),
    c("GETDEL", W, "key", "Get a value and delete the key"),
    c("GETEX", W, "key [EX seconds|PX milliseconds|PERSIST]", "Get a value and change its expiry"),
    c("MGET", R, "key [key ...]", "Values of several keys"),
    c("MSET", W, "key value [key value ...]", "Set several keys"),
    c("MSETNX", W, "key value [key value ...]", "Set several keys only if none exist"),
    c("APPEND", W, "key value", "Append to a string"),
    c("STRLEN", R, "key", "Length of a string value"),
    c("GETRANGE", R, "key start end", "Substring of a string value"),
    c("SETRANGE", W, "key offset value", "Overwrite part of a string"),
    c("INCR", W, "key", "Increment an integer by one"),
    c("INCRBY", W, "key increment", "Increment an integer"),
    c("INCRBYFLOAT", W, "key increment", "Increment a float"),
    c("DECR", W, "key", "Decrement an integer by one"),
    c("DECRBY", W, "key decrement", "Decrement an integer"),
    c("LCS", R, "key1 key2 [LEN] [IDX]", "Longest common substring"),
    // hashes
    c("HGET", R, "key field", "Value of a hash field"),
    c("HSET", W, "key field value [field value ...]", "Set hash fields"),
    c("HSETNX", W, "key field value", "Set a hash field only if it doesn't exist"),
    c("HMGET", R, "key field [field ...]", "Values of several hash fields"),
    c("HMSET", W, "key field value [field value ...]", "Set hash fields (deprecated; use HSET)"),
    c("HGETALL", R, "key", "Every field and value of a hash"),
    c("HKEYS", R, "key", "Field names of a hash"),
    c("HVALS", R, "key", "Values of a hash"),
    c("HLEN", R, "key", "Number of fields in a hash"),
    c("HEXISTS", R, "key field", "Whether a hash field exists"),
    c("HDEL", W, "key field [field ...]", "Delete hash fields"),
    c("HINCRBY", W, "key field increment", "Increment a hash field's integer"),
    c("HINCRBYFLOAT", W, "key field increment", "Increment a hash field's float"),
    c("HSTRLEN", R, "key field", "Length of a hash field's value"),
    c("HRANDFIELD", R, "key [count [WITHVALUES]]", "Random fields of a hash"),
    c("HSCAN", R, "key cursor [MATCH pattern] [COUNT count]", "Iterate over a hash's fields"),
    c("HEXPIRE", W, "key seconds FIELDS numfields field [field ...]", "Expire hash fields"),
    c("HTTL", R, "key FIELDS numfields field [field ...]", "Time to live of hash fields"),
    // lists
    c("LPUSH", W, "key element [element ...]", "Prepend to a list"),
    c("RPUSH", W, "key element [element ...]", "Append to a list"),
    c("LPUSHX", W, "key element [element ...]", "Prepend only if the list exists"),
    c("RPUSHX", W, "key element [element ...]", "Append only if the list exists"),
    c("LPOP", W, "key [count]", "Remove and return the first elements"),
    c("RPOP", W, "key [count]", "Remove and return the last elements"),
    c("LRANGE", R, "key start stop", "Elements of a list (0 -1 for all)"),
    c("LINDEX", R, "key index", "Element at an index"),
    c("LLEN", R, "key", "Length of a list"),
    c("LPOS", R, "key element [RANK rank] [COUNT num-matches]", "Index of matching elements"),
    c("LSET", W, "key index element", "Set the element at an index"),
    c("LINSERT", W, "key BEFORE|AFTER pivot element", "Insert before or after an element"),
    c("LREM", W, "key count element", "Remove matching elements"),
    c("LTRIM", W, "key start stop", "Keep only a range of a list"),
    c("LMOVE", W, "source destination LEFT|RIGHT LEFT|RIGHT", "Move an element between lists"),
    c("RPOPLPUSH", W, "source destination", "Move the last element to another list"),
    c("BLPOP", W, "key [key ...] timeout", "Blocking LPOP"),
    c("BRPOP", W, "key [key ...] timeout", "Blocking RPOP"),
    // sets
    c("SADD", W, "key member [member ...]", "Add members to a set"),
    c("SREM", W, "key member [member ...]", "Remove members from a set"),
    c("SMEMBERS", R, "key", "Every member of a set"),
    c("SISMEMBER", R, "key member", "Whether a member is in a set"),
    c("SMISMEMBER", R, "key member [member ...]", "Whether each member is in a set"),
    c("SCARD", R, "key", "Number of members in a set"),
    c("SRANDMEMBER", R, "key [count]", "Random members of a set"),
    c("SPOP", W, "key [count]", "Remove and return random members"),
    c("SMOVE", W, "source destination member", "Move a member between sets"),
    c("SINTER", R, "key [key ...]", "Intersection of sets"),
    c("SUNION", R, "key [key ...]", "Union of sets"),
    c("SDIFF", R, "key [key ...]", "Difference of sets"),
    c("SINTERCARD", R, "numkeys key [key ...] [LIMIT limit]", "Size of the intersection"),
    c("SINTERSTORE", W, "destination key [key ...]", "Store an intersection"),
    c("SUNIONSTORE", W, "destination key [key ...]", "Store a union"),
    c("SDIFFSTORE", W, "destination key [key ...]", "Store a difference"),
    c("SSCAN", R, "key cursor [MATCH pattern] [COUNT count]", "Iterate over a set's members"),
    // sorted sets
    c("ZADD", W, "key [NX|XX] [GT|LT] [CH] [INCR] score member [score member ...]", "Add members with scores"),
    c("ZREM", W, "key member [member ...]", "Remove members"),
    c("ZRANGE", R, "key start stop [BYSCORE|BYLEX] [REV] [LIMIT offset count] [WITHSCORES]", "Members in a range (0 -1 WITHSCORES for all)"),
    c("ZREVRANGE", R, "key start stop [WITHSCORES]", "Members in a range, highest first"),
    c("ZRANGEBYSCORE", R, "key min max [WITHSCORES] [LIMIT offset count]", "Members with scores in a range"),
    c("ZREVRANGEBYSCORE", R, "key max min [WITHSCORES] [LIMIT offset count]", "Members with scores in a range, highest first"),
    c("ZRANGEBYLEX", R, "key min max [LIMIT offset count]", "Members in a lexicographic range"),
    c("ZSCORE", R, "key member", "Score of a member"),
    c("ZMSCORE", R, "key member [member ...]", "Scores of several members"),
    c("ZRANK", R, "key member [WITHSCORE]", "Rank of a member, lowest first"),
    c("ZREVRANK", R, "key member [WITHSCORE]", "Rank of a member, highest first"),
    c("ZCARD", R, "key", "Number of members"),
    c("ZCOUNT", R, "key min max", "Members with scores in a range"),
    c("ZLEXCOUNT", R, "key min max", "Members in a lexicographic range"),
    c("ZINCRBY", W, "key increment member", "Increment a member's score"),
    c("ZPOPMIN", W, "key [count]", "Remove and return the lowest members"),
    c("ZPOPMAX", W, "key [count]", "Remove and return the highest members"),
    c("ZRANDMEMBER", R, "key [count [WITHSCORES]]", "Random members"),
    c("ZREMRANGEBYRANK", W, "key start stop", "Remove members in a rank range"),
    c("ZREMRANGEBYSCORE", W, "key min max", "Remove members in a score range"),
    c("ZUNIONSTORE", W, "destination numkeys key [key ...]", "Store a union of sorted sets"),
    c("ZINTERSTORE", W, "destination numkeys key [key ...]", "Store an intersection of sorted sets"),
    c("ZSCAN", R, "key cursor [MATCH pattern] [COUNT count]", "Iterate over members and scores"),
    // streams
    c("XADD", W, "key [MAXLEN [~] threshold] *|id field value [field value ...]", "Append an entry to a stream"),
    c("XRANGE", R, "key start end [COUNT count]", "Entries in an ID range (- + for all)"),
    c("XREVRANGE", R, "key end start [COUNT count]", "Entries in an ID range, newest first"),
    c("XLEN", R, "key", "Number of entries in a stream"),
    c("XREAD", R, "[COUNT count] [BLOCK milliseconds] STREAMS key [key ...] id [id ...]", "Read entries from streams"),
    c("XDEL", W, "key id [id ...]", "Delete stream entries"),
    c("XTRIM", W, "key MAXLEN|MINID [~] threshold", "Trim a stream"),
    c("XINFO", R, "STREAM|GROUPS|CONSUMERS key", "Details of a stream"),
    c("XGROUP", W, "CREATE|DESTROY|SETID|CREATECONSUMER|DELCONSUMER key group ...", "Manage consumer groups"),
    c("XREADGROUP", W, "GROUP group consumer [COUNT count] STREAMS key [key ...] id [id ...]", "Read as a consumer group"),
    c("XACK", W, "key group id [id ...]", "Acknowledge entries"),
    c("XPENDING", R, "key group [start end count]", "Pending entries of a group"),
    // HyperLogLog, bitmaps, geo
    c("PFADD", W, "key [element ...]", "Add to a HyperLogLog"),
    c("PFCOUNT", R, "key [key ...]", "Approximate number of unique elements"),
    c("PFMERGE", W, "destkey [sourcekey ...]", "Merge HyperLogLogs"),
    c("SETBIT", W, "key offset value", "Set a bit"),
    c("GETBIT", R, "key offset", "Value of a bit"),
    c("BITCOUNT", R, "key [start end [BYTE|BIT]]", "Number of set bits"),
    c("BITPOS", R, "key bit [start [end]]", "First set or clear bit"),
    c("GEOADD", W, "key longitude latitude member [longitude latitude member ...]", "Add geospatial members"),
    c("GEOPOS", R, "key member [member ...]", "Coordinates of members"),
    c("GEODIST", R, "key member1 member2 [M|KM|FT|MI]", "Distance between members"),
    c("GEOSEARCH", R, "key FROMMEMBER member|FROMLONLAT lon lat BYRADIUS radius M|KM ...", "Members within an area"),
    // JSON (Redis Stack / Redis 8)
    c("JSON.GET", R, "key [path ...]", "Value of a JSON document (RedisJSON)"),
    c("JSON.SET", W, "key path value [NX|XX]", "Set a JSON value (RedisJSON)"),
    c("JSON.DEL", W, "key [path]", "Delete a JSON value (RedisJSON)"),
    c("JSON.TYPE", R, "key [path]", "Type of a JSON value (RedisJSON)"),
    // pub/sub, scripting
    c("PUBLISH", W, "channel message", "Post a message to a channel"),
    c("PUBSUB", R, "CHANNELS|NUMSUB|NUMPAT [argument ...]", "Pub/sub introspection"),
    c("EVAL", W, "script numkeys [key ...] [arg ...]", "Run a Lua script"),
    c("EVALSHA", W, "sha1 numkeys [key ...] [arg ...]", "Run a cached Lua script"),
    c("EVAL_RO", R, "script numkeys [key ...] [arg ...]", "Run a read-only Lua script"),
    c("FCALL", W, "function numkeys [key ...] [arg ...]", "Call a function"),
    c("FCALL_RO", R, "function numkeys [key ...] [arg ...]", "Call a read-only function"),
    // transactions
    c("MULTI", R, "", "Start a transaction (commands queue until EXEC)"),
    c("EXEC", W, "", "Run the queued commands"),
    c("DISCARD", R, "", "Drop the queued commands"),
    c("WATCH", R, "key [key ...]", "Abort the next EXEC if these keys change"),
    c("UNWATCH", R, "", "Forget watched keys"),
    // connection and server
    c("PING", R, "[message]", "Check the connection"),
    c("ECHO", R, "message", "Return the message"),
    c("SELECT", R, "index", "Switch to another logical database"),
    c("DBSIZE", R, "", "Number of keys in the database"),
    c("INFO", R, "[section ...]", "Server information and statistics"),
    c("TIME", R, "", "Server time"),
    c("LASTSAVE", R, "", "Time of the last save to disk"),
    c("ROLE", R, "", "Replication role"),
    c("CLIENT", A, "LIST|INFO|ID|GETNAME|SETNAME|KILL ...", "Inspect and manage client connections"),
    c("CONFIG", A, "GET|SET|RESETSTAT|REWRITE parameter [value]", "Read or change server settings"),
    c("MEMORY", R, "USAGE key|STATS|DOCTOR", "Memory usage"),
    c("SLOWLOG", R, "GET [count]|LEN|RESET", "Slow query log"),
    c("LATENCY", R, "LATEST|HISTORY event|DOCTOR", "Latency monitoring"),
    c("COMMAND", R, "[COUNT|INFO|DOCS|LIST]", "Details about commands"),
    c("ACL", A, "WHOAMI|LIST|USERS|GETUSER|SETUSER|DELUSER ...", "Access control lists"),
    c("MODULE", A, "LIST|LOAD|UNLOAD", "Server modules"),
    c("SAVE", A, "", "Save the dataset to disk (blocks)"),
    c("BGSAVE", A, "", "Save the dataset in the background"),
    c("BGREWRITEAOF", A, "", "Rewrite the append-only file"),
    c("FLUSHDB", A, "[ASYNC|SYNC]", "Delete every key in this database"),
    c("FLUSHALL", A, "[ASYNC|SYNC]", "Delete every key in every database"),
    c("SWAPDB", A, "index1 index2", "Swap two databases"),
    c("SHUTDOWN", A, "[NOSAVE|SAVE]", "Stop the server"),
    c("REPLICAOF", A, "host port|NO ONE", "Make the server a replica"),
    c("DEBUG", A, "subcommand ...", "Debugging commands"),
];

pub fn command(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

/// Split a command line into its arguments, redis-cli style: whitespace
/// separates, `"…"` takes `\n`, `\t`, `\"`, `\\` and `\xHH` escapes, `'…'`
/// is literal (except `\'`).
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };
        let mut arg = String::new();
        if first == '"' || first == '\'' {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == first {
                    closed = true;
                    break;
                }
                if c == '\\' {
                    let Some(e) = chars.next() else { break };
                    if first == '\'' {
                        if e != '\'' {
                            arg.push('\\');
                        }
                        arg.push(e);
                        continue;
                    }
                    match e {
                        'n' => arg.push('\n'),
                        'r' => arg.push('\r'),
                        't' => arg.push('\t'),
                        'b' => arg.push('\u{8}'),
                        'a' => arg.push('\u{7}'),
                        'x' => {
                            let h: String = chars.by_ref().take(2).collect();
                            match u8::from_str_radix(&h, 16) {
                                Ok(b) => arg.push(b as char),
                                Err(_) => return Err(format!("Bad escape \\x{h}")),
                            }
                        }
                        other => arg.push(other),
                    }
                } else {
                    arg.push(c);
                }
            }
            if !closed {
                return Err(format!("Unclosed {first} quote"));
            }
            if chars.peek().is_some_and(|c| !c.is_whitespace()) {
                return Err("A closing quote must be followed by a space".into());
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                arg.push(c);
                chars.next();
            }
        }
        out.push(arg);
    }
    Ok(out)
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//") || t.starts_with("--")
}

/// One command per non-empty, non-comment line.
pub fn split(src: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in src.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim();
        if !trimmed.is_empty() && !is_comment(body) {
            let lead = body.len() - body.trim_start().len();
            out.push(start + lead..start + lead + trimmed.len());
        }
        start += line.len();
    }
    out
}

fn verb(stmt: &str) -> (String, Vec<String>) {
    let args = split_args(stmt).unwrap_or_else(|_| stmt.split_whitespace().map(str::to_string).collect());
    let name = args.first().map(|a| a.to_ascii_uppercase()).unwrap_or_default();
    (name, args)
}

pub fn classify(stmt: &str) -> StatementKind {
    let (name, args) = verb(stmt);
    let sub = args.get(1).map(|a| a.to_ascii_uppercase()).unwrap_or_default();
    // read-only subcommands of admin commands
    match (name.as_str(), sub.as_str()) {
        ("CONFIG", "GET") | ("CLIENT", "LIST" | "INFO" | "ID" | "GETNAME") | ("ACL", "WHOAMI" | "LIST" | "USERS" | "GETUSER" | "CAT") | ("MODULE", "LIST") => return StatementKind::Read,
        _ => {}
    }
    match command(&name).map(|c| c.access) {
        Some(Access::Read) => StatementKind::Read,
        Some(Access::Write) => StatementKind::Write,
        Some(Access::Admin) => StatementKind::Ddl,
        None if name.is_empty() => StatementKind::Other,
        None => StatementKind::Other,
    }
}

pub fn dangers(stmt: &str) -> Vec<Danger> {
    let (name, args) = verb(stmt);
    match name.as_str() {
        "FLUSHDB" | "FLUSHALL" => vec![Danger::Truncate],
        "SHUTDOWN" => vec![Danger::Drop],
        "DEL" | "UNLINK" if args.len() > 1 => vec![Danger::Drop],
        _ => vec![],
    }
}

/// Where the cursor is in a command line: which argument it's typing.
pub struct LineContext<'a> {
    /// Arguments before the one being typed.
    pub before: Vec<String>,
    pub prefix: &'a str,
}

pub fn line_context(line: &str) -> LineContext<'_> {
    let word_start = line.rfind(char::is_whitespace).map(|p| p + 1).unwrap_or(0);
    let before = split_args(&line[..word_start]).unwrap_or_default();
    LineContext { before, prefix: &line[word_start..] }
}

/// Arguments that take a key, by position (0 = the command itself), for
/// commands whose keys aren't simply every argument.
pub fn takes_key(cmd: &str, pos: usize) -> bool {
    let Some(c) = command(cmd) else { return false };
    if c.args.is_empty() {
        return false;
    }
    let first = c.args.split_whitespace().next().unwrap_or("");
    match c.name {
        "MSET" | "MSETNX" => pos % 2 == 1,
        "OBJECT" | "MEMORY" => pos == 2,
        "SCAN" | "SELECT" | "PUBLISH" | "EVAL" | "EVALSHA" | "EVAL_RO" | "FCALL" | "FCALL_RO" | "XREAD" | "XREADGROUP" => false,
        "ZUNIONSTORE" | "ZINTERSTORE" => pos == 1 || pos >= 3,
        _ if c.args.contains("key [key ...]") || c.args.starts_with("key [key") => pos >= 1,
        _ if first == "key" || first == "source" || first == "destination" || first == "key1" => pos == 1 || (pos == 2 && (c.args.contains("destination") || c.args.contains("newkey") || c.args.contains("key2"))),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arguments_like_redis_cli() {
        assert_eq!(split_args(r#"SET greeting "hello world""#).unwrap(), vec!["SET", "greeting", "hello world"]);
        assert_eq!(split_args(r#"SET k "a\"b\n" 'it\'s'"#).unwrap(), vec!["SET", "k", "a\"b\n", "it's"]);
        assert_eq!(split_args("  HGETALL   user:1  ").unwrap(), vec!["HGETALL", "user:1"]);
        assert_eq!(split_args(r#"SET k "\x41""#).unwrap(), vec!["SET", "k", "A"]);
        assert!(split_args(r#"SET k "open"#).is_err());
        assert!(split_args(r#"SET k "a"b"#).is_err());
        assert!(split_args("").unwrap().is_empty());
    }

    #[test]
    fn one_command_per_line() {
        let src = "# users\nHGETALL user:1\n\n  GET counter:page_views  \n// done\nDBSIZE";
        let parts: Vec<&str> = split(src).into_iter().map(|r| &src[r]).collect();
        assert_eq!(parts, vec!["HGETALL user:1", "GET counter:page_views", "DBSIZE"]);
    }

    #[test]
    fn reads_writes_and_admin() {
        assert_eq!(classify("get k"), StatementKind::Read);
        assert_eq!(classify("HGETALL user:1"), StatementKind::Read);
        assert_eq!(classify("SET k v"), StatementKind::Write);
        assert_eq!(classify("FLUSHALL"), StatementKind::Ddl);
        assert_eq!(classify("CONFIG GET maxmemory"), StatementKind::Read);
        assert_eq!(classify("CONFIG SET maxmemory 1mb"), StatementKind::Ddl);
        assert_eq!(classify("SOMEMODULE.CMD x"), StatementKind::Other);
        assert!(classify("UNKNOWN").modifies());
        assert_eq!(dangers("flushdb"), vec![Danger::Truncate]);
        assert!(dangers("GET k").is_empty());
    }

    #[test]
    fn key_positions() {
        assert!(takes_key("GET", 1));
        assert!(!takes_key("GET", 2));
        assert!(takes_key("DEL", 3));
        assert!(takes_key("HGET", 1) && !takes_key("HGET", 2));
        assert!(takes_key("RENAME", 2));
        assert!(!takes_key("SCAN", 1));
        assert!(takes_key("MSET", 1) && !takes_key("MSET", 2) && takes_key("MSET", 3));
    }

    #[test]
    fn commands_are_unique_and_upper_case() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
        assert!(names.iter().all(|n| n.chars().all(|c| !c.is_ascii_lowercase())));
        names.sort();
        let n = names.len();
        names.dedup();
        assert_eq!(n, names.len());
    }
}

#[cfg(test)]
mod lexer_tests {
    use crate::lexer::{tokenize, TokKind};
    use databug_core::Dialect;

    #[test]
    fn commands_are_keywords_only_at_the_start_of_a_line() {
        let src = "# note\nHGET user:1 name\nSET k \"a b\" EX 60\nget GET";
        let toks: Vec<(TokKind, &str)> = tokenize(src, Dialect::Redis).into_iter().filter(|t| t.kind != TokKind::Whitespace).map(|t| (t.kind, &src[t.start..t.end])).collect();
        assert_eq!(toks[0], (TokKind::Comment, "# note"));
        assert_eq!(toks[1], (TokKind::Keyword, "HGET"));
        assert_eq!(toks[2], (TokKind::Ident, "user:1"));
        assert_eq!(toks[5], (TokKind::Ident, "k"));
        assert_eq!(toks[6], (TokKind::String, "\"a b\""));
        assert_eq!(toks[8], (TokKind::Number, "60"));
        assert_eq!(toks[9], (TokKind::Keyword, "get"));
        assert_eq!(toks[10], (TokKind::Ident, "GET"));
        let src = "LIMIT 200;\n\nWHERE p.price > 100;\n\nHGET user:1 ";
        let kinds: Vec<(TokKind, &str)> = tokenize(src, Dialect::Redis).into_iter().filter(|t| t.kind != TokKind::Whitespace).map(|t| (t.kind, &src[t.start..t.end])).collect();
        assert!(kinds.contains(&(TokKind::Keyword, "HGET")), "{kinds:?}");
    }
}
