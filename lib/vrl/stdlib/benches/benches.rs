use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use criterion::{criterion_group, criterion_main, Criterion};
use regex::Regex;
use shared::btreemap;
use vrl::prelude::*;

criterion_group!(
    name = benches;
    // encapsulates CI noise we saw in
    // https://github.com/timberio/vector/pull/6408
    config = Criterion::default().noise_threshold(0.05);
    targets = assert,
              assert_eq,
              ceil,
              compact,
              contains,
              decode_base64,
              decode_percent,
              // TODO: Cannot pass a Path to bench_function
              //del,
              downcase,
              encode_base64,
              encode_key_value,
              encode_json,
              encode_logfmt,
              encode_percent,
              ends_with,
              // TODO: Cannot pass a Path to bench_function
              //exists
              flatten,
              floor,
              format_int,
              format_number,
              format_timestamp,
              get_env_var,
              get_hostname,
              includes,
              ip_aton,
              ip_cidr_contains,
              ip_ntoa,
              ip_subnet,
              ip_to_ipv6,
              ipv6_to_ipv4,
              is_array,
              is_boolean,
              is_float,
              is_integer,
              is_null,
              is_nullish,
              is_object,
              is_regex,
              is_string,
              is_timestamp,
              join,
              length,
              log,
              r#match,
              match_any,
              match_array,
              md5,
              merge,
              // TODO: value is dynamic so we cannot assert equality
              //now,
              parse_apache_log,
              parse_aws_alb_log,
              parse_aws_cloudwatch_log_subscription_message,
              parse_aws_vpc_flow_log,
              parse_common_log,
              parse_csv,
              parse_duration,
              parse_glog,
              parse_datadog_grok,
              parse_grok,
              parse_key_value,
              parse_klog,
              parse_int,
              parse_json,
              parse_nginx_log,
              parse_query_string,
              parse_regex,
              parse_regex_all,
              parse_ruby_hash,
              parse_syslog,
              parse_timestamp,
              parse_tokens,
              parse_url,
              parse_xml,
              push,
              redact,
              replace,
              round,
              sha1,
              sha2,
              sha3,
              slice,
              split,
              starts_with,
              strip_ansi_escape_codes,
              strip_whitespace,
              to_bool,
              to_float,
              to_int,
              to_regex,
              to_string,
              to_syslog_facility,
              to_syslog_level,
              to_syslog_severity,
              to_timestamp,
              to_unix_timestamp,
              truncate,
              // TODO: value is dynamic so we cannot assert equality
              //uuidv4,
              upcase
);
criterion_main!(benches);

bench_function! {
    append => vrl_stdlib::Append;

    arrays {
        args: func_args![value: value!([1, 2, 3]), items: value!([4, 5, 6])],
        want: Ok(value!([1, 2, 3, 4, 5, 6])),
    }
}

bench_function! {
    assert => vrl_stdlib::Assert;

    literal {
        args: func_args![condition: value!(true), message: "must be true"],
        want: Ok(value!(true)),
    }
}

bench_function! {
    assert_eq=> vrl_stdlib::AssertEq;

    literal {
        args: func_args![left: value!(true), right: value!(true), message: "must be true"],
        want: Ok(value!(true)),
    }
}

bench_function! {
    ceil => vrl_stdlib::Ceil;

    literal {
        args: func_args![value: 1234.56725, precision: 4],
        want: Ok(1234.5673),
    }
}

bench_function! {
    compact => vrl_stdlib::Compact;

    array {
        args: func_args![
            value: value!([null, 1, "" ]),
        ],
        want: Ok(value!([ 1 ])),
    }

    map {
        args: func_args![
            value: value!({ "key1": null, "key2":  1, "key3": "" }),
        ],
        want: Ok(value!({ "key2": 1 })),
    }
}

bench_function! {
    contains => vrl_stdlib::Contains;

    case_sensitive {
        args: func_args![value: "abcdefg", substring: "cde", case_sensitive: true],
        want: Ok(value!(true)),
    }

    case_insensitive {
        args: func_args![value: "abcdefg", substring: "CDE", case_sensitive: false],
        want: Ok(value!(true)),
    }
}

bench_function! {
    decode_base64 => vrl_stdlib::DecodeBase64;

    literal {
        args: func_args![value: "c29tZSs9c3RyaW5nL3ZhbHVl"],
        want: Ok("some+=string/value"),
    }
}

bench_function! {
    decode_percent => vrl_stdlib::DecodePercent;

    literal {
        args: func_args![value: "foo%20bar%3F"],
        want: Ok("foo bar?"),
    }
}

bench_function! {
    downcase => vrl_stdlib::Downcase;

    literal {
        args: func_args![value: "FOO"],
        want: Ok("foo")
    }
}

bench_function! {
    encode_base64 => vrl_stdlib::EncodeBase64;

    literal {
        args: func_args![value: "some+=string/value"],
        want: Ok("c29tZSs9c3RyaW5nL3ZhbHVl"),
    }
}

bench_function! {
    encode_key_value => vrl_stdlib::EncodeKeyValue;

    encode_complex_value {
        args: func_args![value:
            btreemap! {
                "msg" => r#"result: {"authz": false, "length": 42}\n"#,
                "severity" => "    panic"
            },
            key_value_delimiter: "==",
            field_delimiter: "!!!"
            ],
        want: Ok(r#"msg=="result: {\"authz\": false, \"length\": 42}\\n"!!!severity=="    panic""#),
    }

    encode_key_value {
        args: func_args![value:
            btreemap! {
                "mow" => "vvo",
                "vvo" => "pkc",
                "pkc" => "hrb",
                "hrb" => "tsn",
                "tsn" => "can",
                "can" => "pnh",
                "pnh" => "sin",
                "sin" => "syd"
            },
            key_value_delimiter: ":",
            field_delimiter: ","
        ],
        want: Ok(r#"can:pnh,hrb:tsn,mow:vvo,pkc:hrb,pnh:sin,sin:syd,tsn:can,vvo:pkc"#),
    }

    fields_ordering {
        args: func_args![value:
            btreemap! {
                "mow" => "vvo",
                "vvo" => "pkc",
                "pkc" => "hrb",
                "hrb" => "tsn",
                "tsn" => "can",
                "can" => "pnh",
                "pnh" => "sin",
                "sin" => "syd"
            },
            fields_ordering: value!(["mow", "vvo", "pkc", "hrb", "tsn", "can", "pnh", "sin"]),
            key_value_delimiter: ":",
            field_delimiter: ","
        ],
        want: Ok(r#"mow:vvo,vvo:pkc,pkc:hrb,hrb:tsn,tsn:can,can:pnh,pnh:sin,sin:syd"#),
    }
}

bench_function! {
    encode_json => vrl_stdlib::EncodeJson;

    map {
        args: func_args![value: value![{"field": "value"}]],
        want: Ok(r#"{"field":"value"}"#),
    }
}

bench_function! {
    encode_logfmt => vrl_stdlib::EncodeLogfmt;

    string_with_characters_to_escape {
        args: func_args![value:
            btreemap! {
                "lvl" => "info",
                "msg" => r#"payload: {"code": 200}\n"#
            }],
        want: Ok(r#"lvl=info msg="payload: {\"code\": 200}\\n""#),
    }

    fields_ordering {
        args: func_args![value:
            btreemap! {
                "lvl" => "info",
                "msg" => "This is a log message",
                "log_id" => 12345,
            },
            fields_ordering: value!(["lvl", "msg"])
        ],
        want: Ok(r#"lvl=info msg="This is a log message" log_id=12345"#),
    }
}

bench_function! {
    encode_percent => vrl_stdlib::EncodePercent;

    non_alphanumeric {
        args: func_args![value: r#"foo bar?"#],
        want: Ok(r#"foo%20bar%3F"#),
    }

    controls {
        args: func_args![value: r#"foo bar"#, ascii_set: "CONTROLS"],
        want: Ok(r#"foo %14bar"#),
    }
}

bench_function! {
    ends_with => vrl_stdlib::EndsWith;

    case_sensitive {
        args: func_args![value: "abcdefg", substring: "efg", case_sensitive: true],
        want: Ok(value!(true)),
    }

    case_insensitive {
        args: func_args![value: "abcdefg", substring: "EFG", case_sensitive: false],
        want: Ok(value!(true)),
    }
}

bench_function! {
    flatten => vrl_stdlib::Flatten;

    nested_map {
        args: func_args![value: value!({parent: {child1: 1, child2: 2}, key: "val"})],
        want: Ok(value!({"parent.child1": 1, "parent.child2": 2, key: "val"})),
    }

    nested_array {
        args: func_args![value: value!([42, [43, 44]])],
        want: Ok(value!([42, 43, 44])),
    }

    map_and_array {
        args: func_args![value: value!({
            "parent": {
                "child1": [1, [2, 3]],
                "child2": {"grandchild1": 1, "grandchild2": [1, [2, 3], 4]},
            },
            "key": "val",
        })],
        want: Ok(value!({
            "parent.child1": [1, [2, 3]],
            "parent.child2.grandchild1": 1,
            "parent.child2.grandchild2": [1, [2, 3], 4],
            "key": "val",
        })),
    }
}

bench_function! {
    floor  => vrl_stdlib::Floor;

    literal {
        args: func_args![value: 1234.56725, precision: 4],
        want: Ok(1234.5672),
    }
}

bench_function! {
    format_int => vrl_stdlib::FormatInt;

    decimal {
        args: func_args![value: 42],
        want: Ok("42"),
    }

    hexidecimal {
        args: func_args![value: 42, base: 16],
        want: Ok(value!("2a")),
    }
}

bench_function! {
    format_number => vrl_stdlib::FormatNumber;

    literal {
        args: func_args![
            value: 11222333444.56789,
            scale: 3,
            decimal_separator: ",",
            grouping_separator: "."
        ],
        want: Ok("11.222.333.444,567"),
    }
}

bench_function! {
    format_timestamp => vrl_stdlib::FormatTimestamp;

    iso_6801 {
        args: func_args![value: Utc.timestamp(10, 0), format: "%+"],
        want: Ok("1970-01-01T00:00:10+00:00"),
    }
}

bench_function! {
    get_env_var => vrl_stdlib::GetEnvVar;

    // CARGO is set by `cargo`
    get {
        args: func_args![name: "CARGO"],
        want: Ok(env!("CARGO")),
    }
}

bench_function! {
    get_hostname => vrl_stdlib::GetHostname;

    get {
        args: func_args![],
        want: Ok(hostname::get().unwrap().to_string_lossy()),
    }
}

bench_function! {
    includes => vrl_stdlib::Includes;

    mixed_included_string {
        args: func_args![value: value!(["foo", 1, true, [1,2,3]]), item: value!("foo")],
        want: Ok(value!(true)),
    }
}

bench_function! {
    ip_aton => vrl_stdlib::IpAton;

    valid {
        args: func_args![value: "1.2.3.4"],
        want: Ok(value!(67305985)),
    }
}

bench_function! {
    ip_cidr_contains => vrl_stdlib::IpCidrContains;

    ipv4 {
        args: func_args![cidr: "192.168.0.0/16", value: "192.168.10.32"],
        want: Ok(true),
    }

    ipv6 {
        args: func_args![cidr: "2001:4f8:3:ba::/64", value: "2001:4f8:3:ba:2e0:81ff:fe22:d1f1"],
        want: Ok(true),
    }
}

bench_function! {
    ip_ntoa => vrl_stdlib::IpNtoa;

    valid {
        args: func_args![value: 67305985],
        want: Ok(value!("1.2.3.4")),
    }
}

bench_function! {
    ip_subnet => vrl_stdlib::IpSubnet;

    ipv4_mask {
        args: func_args![value: "192.168.10.23", subnet: "255.255.0.0"],
        want: Ok("192.168.0.0"),
    }

    ipv4_prefix {
        args: func_args![value: "192.168.10.23", subnet: "/16"],
        want: Ok("192.168.0.0"),
    }

    ipv6_mask {
        args: func_args![value: "2400:6800:4003:c02::64", subnet: "ff00::"],
        want: Ok("2400::"),
    }

    ipv6_prefix {
        args: func_args![value: "2400:6800:4003:c02::64", subnet: "/16"],
        want: Ok("2400::"),
    }
}

bench_function! {
    ip_to_ipv6 => vrl_stdlib::IpToIpv6;

    ipv4 {
        args: func_args![value: "192.168.0.1"],
        want: Ok("::ffff:192.168.0.1"),
    }

    ipv6 {
        args: func_args![value: "2404:6800:4003:c02::64"],
        want: Ok("2404:6800:4003:c02::64"),
    }
}

bench_function! {
    ipv6_to_ipv4 => vrl_stdlib::Ipv6ToIpV4;

    ipv6 {
        args: func_args![value: "::ffff:192.168.0.1"],
        want: Ok("192.168.0.1"),
    }

    ipv4 {
        args: func_args![value: "198.51.100.16"],
        want: Ok("198.51.100.16"),
    }
}

bench_function! {
    is_array => vrl_stdlib::IsArray;

    string {
        args: func_args![value: "foobar"],
        want: Ok(false),
    }

    array {
        args: func_args![value: value!([1, 2, 3])],
        want: Ok(true),
    }
}

bench_function! {
    is_boolean => vrl_stdlib::IsBoolean;

    string {
        args: func_args![value: "foobar"],
        want: Ok(false),
    }

    boolean {
        args: func_args![value: true],
        want: Ok(true),
    }
}

bench_function! {
    is_float => vrl_stdlib::IsFloat;

    array {
        args: func_args![value: value!([1, 2, 3])],
        want: Ok(false),
    }

    float {
        args: func_args![value: 0.577],
        want: Ok(true),
    }
}

bench_function! {
    is_integer => vrl_stdlib::IsInteger;

    integer {
        args: func_args![value: 1701],
        want: Ok(true),
    }

    object {
        args: func_args![value: value!({"foo": "bar"})],
        want: Ok(false),
    }
}

bench_function! {
    is_null => vrl_stdlib::IsNull;

    string {
        args: func_args![value: "foobar"],
        want: Ok(false),
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok(true),
    }
}

bench_function! {
    is_nullish => vrl_stdlib::IsNullish;

    whitespace {
        args: func_args![value: "         "],
        want: Ok(true),
    }

    hyphen {
        args: func_args![value: "-"],
        want: Ok(true),
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok(true),
    }

    not_empty {
        args: func_args![value: "foo"],
        want: Ok(false),
    }
}

bench_function! {
    is_object => vrl_stdlib::IsObject;

    integer {
        args: func_args![value: 1701],
        want: Ok(false),
    }

    object {
        args: func_args![value: value!({"foo": "bar"})],
        want: Ok(true),
    }
}

bench_function! {
    is_regex => vrl_stdlib::IsRegex;

    regex {
        args: func_args![value: value!(Regex::new(r"\d+").unwrap())],
        want: Ok(true),
    }

    object {
        args: func_args![value: value!({"foo": "bar"})],
        want: Ok(false),
    }
}

bench_function! {
    is_string => vrl_stdlib::IsString;

    string {
        args: func_args![value: "foobar"],
        want: Ok(true),
    }

    array {
        args: func_args![value: value!([1, 2, 3])],
        want: Ok(false),
    }
}

bench_function! {
    is_timestamp => vrl_stdlib::IsTimestamp;

    string {
        args: func_args![value: Utc.ymd(2021, 1, 1).and_hms_milli(0, 0, 0, 0)],
        want: Ok(true),
    }

    array {
        args: func_args![value: value!([1, 2, 3])],
        want: Ok(false),
    }
}

bench_function! {
    join => vrl_stdlib::Join;

    literal {
        args: func_args![value: value!(["hello", "world"]), separator: " "],
        want: Ok("hello world"),
    }
}

bench_function! {
    length => vrl_stdlib::Length;

    map {
        args: func_args![value: value!({foo: "bar", baz: true, baq: [1, 2, 3]})],
        want: Ok(3),
    }

    array {
        args: func_args![value: value!([1, 2, 3, 4, true, "hello"])],
        want: Ok(value!(6)),
    }

    string {
        args: func_args![value: "hello world"],
        want: Ok(value!(11))
    }
}

// TODO: Ensure tracing is enabled
bench_function! {
    log => vrl_stdlib::Log;

    literal {
        args: func_args![value: "ohno"],
        want: Ok(value!(null)),
    }
}

bench_function! {
    r#match => vrl_stdlib::Match;

    simple {
        args: func_args![value: "foo 2 bar", pattern: Regex::new("foo \\d bar").unwrap()],
        want: Ok(true),
    }
}

bench_function! {
    match_any => vrl_stdlib::MatchAny;

    simple {
        args: func_args![value: "foo 2 bar", patterns: vec![Regex::new(r"foo \d bar").unwrap()]],
        want: Ok(true),
    }
}

bench_function! {
    match_array => vrl_stdlib::MatchArray;

    single_match {
        args: func_args![
            value: value!(["foo 1 bar"]),
            pattern: Regex::new(r"foo \d bar").unwrap(),
        ],
        want: Ok(true),
    }

    no_match {
        args: func_args![
            value: value!(["foo x bar"]),
            pattern: Regex::new(r"foo \d bar").unwrap(),
        ],
        want: Ok(false),
    }

    some_match {
        args: func_args![
            value: value!(["foo 2 bar", "foo 3 bar", "foo 4 bar", "foo 5 bar"]),
            pattern: Regex::new(r"foo \d bar").unwrap(),
        ],
        want: Ok(true),
    }

    all_match {
        args: func_args![
            value: value!(["foo 2 bar", "foo 3 bar", "foo 4 bar", "foo 5 bar"]),
            pattern: Regex::new(r"foo \d bar").unwrap(),
            all: value!(true)
        ],
        want: Ok(true),
    }

    not_all_match {
        args: func_args![
            value: value!(["foo 2 bar", "foo 3 bar", "foo 4 bar", "foo x bar"]),
            pattern: Regex::new(r"foo \d bar").unwrap(),
            all: value!(true)
        ],
        want: Ok(false),
    }
}

bench_function! {
    md5  => vrl_stdlib::Md5;

    literal {
        args: func_args![value: "foo"],
        want: Ok("acbd18db4cc2f85cedef654fccc4a4d8"),
    }
}

bench_function! {
    merge => vrl_stdlib::Merge;

    simple {
        args: func_args![
            to: value!({ "key1": "val1" }),
            from: value!({ "key2": "val2" }),
        ],
        want: Ok(value!({
            "key1": "val1",
            "key2": "val2",
        }))
    }

    shallow {
        args: func_args![
            to: value!({
                "key1": "val1",
                "child": { "grandchild1": "val1" },
            }),
            from: value!({
                "key2": "val2",
                "child": { "grandchild2": "val2" },
            })
        ],
        want: Ok(value!({
            "key1": "val1",
            "key2": "val2",
            "child": { "grandchild2": "val2" },
        }))
    }

    deep {
        args: func_args![
            to: value!({
                "key1": "val1",
                "child": { "grandchild1": "val1" },
            }),
            from: value!({
                "key2": "val2",
                "child": { "grandchild2": "val2" },
            }),
            deep: true
        ],
        want: Ok(value!({
            "key1": "val1",
            "key2": "val2",
            "child": {
                "grandchild1": "val1",
                "grandchild2": "val2",
            },
        }))
    }
}

bench_function! {
    parse_aws_alb_log => vrl_stdlib::ParseAwsAlbLog;

    literal {
        args: func_args![
            value: r#"http 2018-07-02T22:23:00.186641Z app/my-loadbalancer/50dc6c495c0c9188 192.168.131.39:2817 10.0.0.1:80 0.000 0.001 0.000 200 200 34 366 "GET http://www.example.com:80/ HTTP/1.1" "curl/7.46.0" - - arn:aws:elasticloadbalancing:us-east-2:123456789012:targetgroup/my-targets/73e2d6bc24d8a067 "Root=1-58337262-36d228ad5d99923122bbe354" "-" "-" 0 2018-07-02T22:22:48.364000Z "forward" "-" "-" 10.0.0.1:80 200 "-" "-""#,
        ],
        want: Ok(value!({
            "actions_executed": "forward",
            "chosen_cert_arn": null,
            "classification": null,
            "classification_reason": null,
            "client_host": "192.168.131.39:2817",
            "domain_name": null,
            "elb": "app/my-loadbalancer/50dc6c495c0c9188",
            "elb_status_code": "200",
            "error_reason": null,
            "matched_rule_priority": "0",
            "received_bytes": 34,
            "redirect_url": null,
            "request_creation_time": "2018-07-02T22:22:48.364000Z",
            "request_method": "GET",
            "request_processing_time": 0.0,
            "request_protocol": "HTTP/1.1",
            "request_url": "http://www.example.com:80/",
            "response_processing_time": 0.0,
            "sent_bytes": 366,
            "ssl_cipher": null,
            "ssl_protocol": null,
            "target_group_arn": "arn:aws:elasticloadbalancing:us-east-2:123456789012:targetgroup/my-targets/73e2d6bc24d8a067",
            "target_host": "10.0.0.1:80",
            "target_port_list": ["10.0.0.1:80"],
            "target_processing_time": 0.001,
            "target_status_code": "200",
            "target_status_code_list": ["200"],
            "timestamp": "2018-07-02T22:23:00.186641Z",
            "trace_id": "Root=1-58337262-36d228ad5d99923122bbe354",
            "type": "http",
            "user_agent": "curl/7.46.0"
        })),
    }
}

bench_function! {
    parse_aws_cloudwatch_log_subscription_message => vrl_stdlib::ParseAwsCloudWatchLogSubscriptionMessage;

    literal {
        args: func_args![value: r#"
{
    "messageType": "DATA_MESSAGE",
    "owner": "071959437513",
    "logGroup": "/jesse/test",
    "logStream": "test",
    "subscriptionFilters": [
    "Destination"
    ],
    "logEvents": [
    {
        "id": "35683658089614582423604394983260738922885519999578275840",
        "timestamp": 1600110569039,
        "message": "{\"bytes\":26780,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"157.130.216.193\",\"method\":\"PUT\",\"protocol\":\"HTTP/1.0\",\"referer\":\"https://www.principalcross-platform.io/markets/ubiquitous\",\"request\":\"/expedite/convergence\",\"source_type\":\"stdin\",\"status\":301,\"user-identifier\":\"-\"}"
    },
    {
        "id": "35683658089659183914001456229543810359430816722590236673",
        "timestamp": 1600110569041,
        "message": "{\"bytes\":17707,\"datetime\":\"14/Sep/2020:11:45:41 -0400\",\"host\":\"109.81.244.252\",\"method\":\"GET\",\"protocol\":\"HTTP/2.0\",\"referer\":\"http://www.investormission-critical.io/24/7/vortals\",\"request\":\"/scale/functionalities/optimize\",\"source_type\":\"stdin\",\"status\":502,\"user-identifier\":\"feeney1708\"}"
    }
    ]
}
"#],
        want: Ok(value!({
            "owner":  "071959437513",
            "message_type":  "DATA_MESSAGE",
            "log_group":  "/jesse/test",
            "log_stream":  "test",
            "subscription_filters":  ["Destination"],
            "log_events": [{
                "id":  "35683658089614582423604394983260738922885519999578275840",
                "timestamp":  (Utc.timestamp(1600110569, 39000000)),
                "message":  r#"{"bytes":26780,"datetime":"14/Sep/2020:11:45:41 -0400","host":"157.130.216.193","method":"PUT","protocol":"HTTP/1.0","referer":"https://www.principalcross-platform.io/markets/ubiquitous","request":"/expedite/convergence","source_type":"stdin","status":301,"user-identifier":"-"}"#,
            }, {
                "id":  "35683658089659183914001456229543810359430816722590236673",
                "timestamp":  (Utc.timestamp(1600110569, 41000000)),
                "message":  r#"{"bytes":17707,"datetime":"14/Sep/2020:11:45:41 -0400","host":"109.81.244.252","method":"GET","protocol":"HTTP/2.0","referer":"http://www.investormission-critical.io/24/7/vortals","request":"/scale/functionalities/optimize","source_type":"stdin","status":502,"user-identifier":"feeney1708"}"#,
            }]
        }))
    }
}

bench_function! {
    parse_aws_vpc_flow_log => vrl_stdlib::ParseAwsVpcFlowLog;

    literal {
        args: func_args![
            value:"3 eni-33333333333333333 123456789010 vpc-abcdefab012345678 subnet-22222222bbbbbbbbb i-01234567890123456 10.20.33.164 10.40.2.236 39812 80 6 3 IPv4 10.20.33.164 10.40.2.236 ACCEPT OK",
            format: "version interface_id account_id vpc_id subnet_id instance_id srcaddr dstaddr srcport dstport protocol tcp_flags type pkt_srcaddr pkt_dstaddr action log_status",
        ],
        want: Ok(value!({
            "account_id": 123456789010i64,
            "action": "ACCEPT",
            "dstaddr": "10.40.2.236",
            "dstport": 80,
            "instance_id": "i-01234567890123456",
            "interface_id": "eni-33333333333333333",
            "log_status": "OK",
            "pkt_dstaddr": "10.40.2.236",
            "pkt_srcaddr": "10.20.33.164",
            "protocol": 6,
            "srcaddr": "10.20.33.164",
            "srcport": 39812,
            "subnet_id": "subnet-22222222bbbbbbbbb",
            "tcp_flags": 3,
            "type": "IPv4",
            "version": 3,
            "vpc_id": "vpc-abcdefab012345678",
        })),
    }
}

bench_function! {
    parse_apache_log => vrl_stdlib::ParseApacheLog;

    common {
        args: func_args![value: r#"127.0.0.1 bob frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 200 2326"#,
                         format: "common"
        ],
        want: Ok(value!({
            "host": "127.0.0.1",
            "identity": "bob",
            "user": "frank",
            "timestamp": (DateTime::parse_from_rfc3339("2000-10-10T20:55:36Z").unwrap().with_timezone(&Utc)),
            "message": "GET /apache_pb.gif HTTP/1.0",
            "method": "GET",
            "path": "/apache_pb.gif",
            "protocol": "HTTP/1.0",
            "status": 200,
            "size": 2326,
        })),
    }

    combined {
        args: func_args![value: r#"127.0.0.1 bob frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 200 2326 "http://www.seniorinfomediaries.com/vertical/channels/front-end/bandwidth" "Mozilla/5.0 (X11; Linux i686; rv:5.0) Gecko/1945-10-12 Firefox/37.0""#,
                         format: "combined"
        ],
        want: Ok(value!({
            "agent": "Mozilla/5.0 (X11; Linux i686; rv:5.0) Gecko/1945-10-12 Firefox/37.0",
            "host": "127.0.0.1",
            "identity": "bob",
            "user": "frank",
            "referrer": "http://www.seniorinfomediaries.com/vertical/channels/front-end/bandwidth",
            "timestamp": (DateTime::parse_from_rfc3339("2000-10-10T20:55:36Z").unwrap().with_timezone(&Utc)),
            "message": "GET /apache_pb.gif HTTP/1.0",
            "method": "GET",
            "path": "/apache_pb.gif",
            "protocol": "HTTP/1.0",
            "status": 200,
            "size": 2326,
        })),
    }

    error {
        args: func_args![value: r#"[01/Mar/2021:12:00:19 +0000] [ab:alert] [pid 4803:tid 3814] [client 147.159.108.175:24259] I will bypass the haptic COM bandwidth, that should matrix the CSS driver!"#,
                         format: "error"
        ],
        want: Ok(value!({
            "client": "147.159.108.175",
            "message": "I will bypass the haptic COM bandwidth, that should matrix the CSS driver!",
            "module": "ab",
            "pid": 4803,
            "port": 24259,
            "severity": "alert",
            "thread": "3814",
            "timestamp": (DateTime::parse_from_rfc3339("2021-03-01T12:00:19Z").unwrap().with_timezone(&Utc)),
        })),
    }
}

bench_function! {
    parse_common_log => vrl_stdlib::ParseCommonLog;

    literal {
        args: func_args![value: r#"127.0.0.1 bob frank [10/Oct/2000:13:55:36 -0700] "GET /apache_pb.gif HTTP/1.0" 200 2326"#],
        want: Ok(value!({
            "host": "127.0.0.1",
            "identity": "bob",
            "user": "frank",
            "timestamp": (DateTime::parse_from_rfc3339("2000-10-10T20:55:36Z").unwrap().with_timezone(&Utc)),
            "message": "GET /apache_pb.gif HTTP/1.0",
            "method": "GET",
            "path": "/apache_pb.gif",
            "protocol": "HTTP/1.0",
            "status": 200,
            "size": 2326,
        })),
    }
}

bench_function! {
    parse_csv => vrl_stdlib::ParseCsv;

    literal {
        args: func_args![value: "foo,bar"],
        want: Ok(value!(["foo","bar"]))
    }
}

bench_function! {
    parse_duration => vrl_stdlib::ParseDuration;

    literal {
        args: func_args![value: "1005ms", unit: "s"],
        want: Ok(1.005),
    }
}

bench_function! {
    parse_glog  => vrl_stdlib::ParseGlog;

    literal {
        args: func_args![value: "I20210131 14:48:54.411655 15520 main.c++:9] Hello world!"],
        want: Ok(value!({
            "level": "info",
            "timestamp": (DateTime::parse_from_rfc3339("2021-01-31T14:48:54.411655Z").unwrap().with_timezone(&Utc)),
            "id": 15520,
            "file": "main.c++",
            "line": 9,
            "message": "Hello world!",
        })),
    }
}

bench_function! {
    parse_grok => vrl_stdlib::ParseGrok;

    simple {
        args: func_args![
            value: "2020-10-02T23:22:12.223222Z info Hello world",
            pattern: "%{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:message}",
            remove_empty: false,
        ],
        want: Ok(value!({
            "timestamp": "2020-10-02T23:22:12.223222Z",
            "level": "info",
            "message": "Hello world",
        })),
    }
}

bench_function! {
    parse_datadog_grok => vrl_stdlib::ParseDatadogGrok;

    simple {
        args: func_args![
            value: "2020-10-02T23:22:12.223222Z info Hello world",
            parsing_rules: vec!["test_rule %{TIMESTAMP_ISO8601:timestamp} %{LOGLEVEL:level} %{GREEDYDATA:message}"],
        ],
        want: Ok(value!({
            "timestamp": "2020-10-02T23:22:12.223222Z",
            "level": "info",
            "message": "Hello world",
        })),
    }
}

bench_function! {
    parse_int => vrl_stdlib::ParseInt;

    decimal {
        args: func_args![value: "-42"],
        want: Ok(-42),
    }

    hexidecimal {
        args: func_args![value: "0x2a"],
        want: Ok(42),
    }

    explicit_hexidecimal {
        args: func_args![value: "2a", base: 16],
        want: Ok(42),
    }
}

bench_function! {
    parse_json => vrl_stdlib::ParseJson;

    map {
        args: func_args![value: r#"{"key": "value"}"#],
        want: Ok(value!({key: "value"})),
    }
}

bench_function! {
    parse_key_value => vrl_stdlib::ParseKeyValue;

    logfmt {
        args: func_args! [
            value: r#"level=info msg="Stopping all fetchers" tag=stopping_fetchers id=ConsumerFetcherManager-1382721708341 module=kafka.consumer.ConsumerFetcherManager"#
        ],
        want: Ok(value!({
            level: "info",
            msg: "Stopping all fetchers",
            tag: "stopping_fetchers",
            id: "ConsumerFetcherManager-1382721708341",
            module: "kafka.consumer.ConsumerFetcherManager"
        }))
    }

    standalone_key_disabled {
        args: func_args! [
            value: r#"level=info msg="Stopping all fetchers" tag=stopping_fetchers id=ConsumerFetcherManager-1382721708341 module=kafka.consumer.ConsumerFetcherManager"#,
            accept_standalone_key: false
        ],
        want: Ok(value!({
            level: "info",
            msg: "Stopping all fetchers",
            tag: "stopping_fetchers",
            id: "ConsumerFetcherManager-1382721708341",
            module: "kafka.consumer.ConsumerFetcherManager"
        }))
    }
}

bench_function! {
    parse_klog  => vrl_stdlib::ParseKlog;

    literal {
        args: func_args![value: "I0505 17:59:40.692994   28133 klog.go:70] hello from klog"],
        want: Ok(btreemap! {
            "level" => "info",
            "timestamp" => Value::Timestamp(DateTime::parse_from_rfc3339(&format!("{}-05-05T17:59:40.692994Z", Utc::now().year())).unwrap().into()),
            "id" => 28133,
            "file" => "klog.go",
            "line" => 70,
            "message" => "hello from klog",
        }),
    }
}

bench_function! {
    parse_nginx_log => vrl_stdlib::ParseNginxLog;

    combined {
        args: func_args![
            value: r#"172.17.0.1 alice - [01/Apr/2021:12:02:31 +0000] "POST /not-found HTTP/1.1" 404 153 "http://localhost/somewhere" "Mozilla/5.0 (Windows NT 6.1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/72.0.3626.119 Safari/537.36" "2.75""#,
            format: "combined",
        ],
        want: Ok(value!({
            "client": "172.17.0.1",
            "user": "alice",
            "timestamp": (DateTime::parse_from_rfc3339("2021-04-01T12:02:31Z").unwrap().with_timezone(&Utc)),
            "request": "POST /not-found HTTP/1.1",
            "method": "POST",
            "path": "/not-found",
            "protocol": "HTTP/1.1",
            "status": 404,
            "size": 153,
            "referer": "http://localhost/somewhere",
            "agent": "Mozilla/5.0 (Windows NT 6.1) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/72.0.3626.119 Safari/537.36",
            "compression": "2.75",
        })),
    }

    error {
        args: func_args![value: r#"2021/04/01 13:02:31 [error] 31#31: *1 open() "/usr/share/nginx/html/not-found" failed (2: No such file or directory), client: 172.17.0.1, server: localhost, request: "POST /not-found HTTP/1.1", host: "localhost:8081""#,
                         format: "error"
        ],
        want: Ok(value!({
            "timestamp": (DateTime::parse_from_rfc3339("2021-04-01T13:02:31Z").unwrap().with_timezone(&Utc)),
            "severity": "error",
            "pid": 31,
            "tid": 31,
            "cid": 1,
            "message": "open() \"/usr/share/nginx/html/not-found\" failed (2: No such file or directory)",
            "client": "172.17.0.1",
            "server": "localhost",
            "request": "POST /not-found HTTP/1.1",
            "host": "localhost:8081",
        })),
    }
}

bench_function! {
    parse_query_string => vrl_stdlib::ParseQueryString;

    literal {
        args: func_args![value: "foo=%2B1&bar=2"],
        want: Ok(value!({
            foo: "+1",
            bar: "2",
        }))
    }
}

bench_function! {
    parse_regex => vrl_stdlib::ParseRegex;

    matches {
        args: func_args! [
            value: "5.86.210.12 - zieme4647 5667 [19/06/2019:17:20:49 -0400] \"GET /embrace/supply-chains/dynamic/vertical\" 201 20574",
            pattern: Regex::new(r#"^(?P<host>[\w\.]+) - (?P<user>[\w]+) (?P<bytes_in>[\d]+) \[(?P<timestamp>.*)\] "(?P<method>[\w]+) (?P<path>.*)" (?P<status>[\d]+) (?P<bytes_out>[\d]+)$"#)
                .unwrap(),
            numeric_groups: true
        ],
        want: Ok(value!({
            "bytes_in": "5667",
            "host": "5.86.210.12",
            "user": "zieme4647",
            "timestamp": "19/06/2019:17:20:49 -0400",
            "method": "GET",
            "path": "/embrace/supply-chains/dynamic/vertical",
            "status": "201",
            "bytes_out": "20574",
            "0": "5.86.210.12 - zieme4647 5667 [19/06/2019:17:20:49 -0400] \"GET /embrace/supply-chains/dynamic/vertical\" 201 20574",
            "1": "5.86.210.12",
            "2": "zieme4647",
            "3": "5667",
            "4": "19/06/2019:17:20:49 -0400",
            "5": "GET",
            "6": "/embrace/supply-chains/dynamic/vertical",
            "7": "201",
            "8": "20574",
        }))
    }

    single_match {
        args: func_args! [
            value: "first group and second group",
            pattern: Regex::new(r#"(?P<number>.*?) group"#).unwrap()
        ],
        want: Ok(value!({
            "number": "first",
        }))
    }
}

bench_function! {
    parse_regex_all => vrl_stdlib::ParseRegexAll;

    matches {
        args: func_args![
            value: "apples and carrots, peaches and peas",
            pattern: Regex::new(r#"(?P<fruit>[\w\.]+) and (?P<veg>[\w]+)"#).unwrap(),
            numeric_groups: true
        ],
        want: Ok(value!([
                {
                    "fruit": "apples",
                    "veg": "carrots",
                    "0": "apples and carrots",
                    "1": "apples",
                    "2": "carrots"
                },
                {
                    "fruit": "peaches",
                    "veg": "peas",
                    "0": "peaches and peas",
                    "1": "peaches",
                    "2": "peas"
                }]))
    }
}

bench_function! {
    parse_ruby_hash => vrl_stdlib::ParseRubyHash;

    matches {
        args: func_args![
            value: r#"{ "test" => "value", "testNum" => 0.2, "testObj" => { "testBool" => true } }"#,
        ],
        want: Ok(value!({
            test: "value",
            testNum: 0.2,
            testObj: {
                testBool: true,
            }
        }))
    }
}

bench_function! {
    parse_syslog => vrl_stdlib::ParseSyslog;

    rfc3164 {
        args: func_args![
            value: r#"<190>Dec 28 2020 16:49:07 plertrood-thinkpad-x220 nginx: 127.0.0.1 - - [28/Dec/2019:16:49:07 +0000] "GET / HTTP/1.1" 304 0 "-" "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:71.0) Gecko/20100101 Firefox/71.0""#
        ],
        want: Ok(value!({
            "severity": "info",
            "facility": "local7",
            "timestamp": (Local.ymd(2020, 12, 28).and_hms_milli(16, 49, 7, 0).with_timezone(&Utc)),
            "hostname": "plertrood-thinkpad-x220",
            "appname": "nginx",
            "message": r#"127.0.0.1 - - [28/Dec/2019:16:49:07 +0000] "GET / HTTP/1.1" 304 0 "-" "Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:71.0) Gecko/20100101 Firefox/71.0""#,
        }))
    }

    rfc5424 {
        args: func_args![value: r#"<13>1 2020-03-13T20:45:38.119Z dynamicwireless.name non 2426 ID931 [exampleSDID@32473 iut="3" eventSource= "Application" eventID="1011"] Try to override the THX port, maybe it will reboot the neural interface!"#],
        want: Ok(value!({
            "severity": "notice",
            "facility": "user",
            "timestamp": (Utc.ymd(2020, 3, 13).and_hms_milli(20, 45, 38, 119)),
            "hostname": "dynamicwireless.name",
            "appname": "non",
            "procid": 2426,
            "msgid": "ID931",
            "exampleSDID@32473.iut": "3",
            "exampleSDID@32473.eventSource": "Application",
            "exampleSDID@32473.eventID": "1011",
            "message": "Try to override the THX port, maybe it will reboot the neural interface!",
            "version": 1,
        }))
    }
}

bench_function! {
    parse_timestamp => vrl_stdlib::ParseTimestamp;

    rfc3339 {
        args: func_args![value: "2001-07-08T00:34:60.026490+09:30", format: "%+"],
        want: Ok(DateTime::parse_from_rfc3339("2001-07-08T00:34:60.026490+09:30").unwrap().with_timezone(&Utc))
    }

    rfc2822 {
        args: func_args![value: "Wed, 16 Oct 2019 12:00:00 +0000", format: "%a, %e %b %Y %T %z"],
        want: Ok(DateTime::parse_from_rfc2822("Wed, 16 Oct 2019 12:00:00 +0000").unwrap().with_timezone(&Utc))
    }
}

bench_function! {
    parse_tokens => vrl_stdlib::ParseTokens;

    literal {
        args: func_args![value: "217.250.207.207 - - [07/Sep/2020:16:38:00 -0400] \"DELETE /deliverables/next-generation/user-centric HTTP/1.1\" 205 11881"],
        want: Ok(value!([
            "217.250.207.207",
            null,
            null,
            "07/Sep/2020:16:38:00 -0400",
            "DELETE /deliverables/next-generation/user-centric HTTP/1.1",
            "205",
            "11881",
        ])),
    }
}

bench_function! {
    parse_url => vrl_stdlib::ParseUrl;

    literal {
        args: func_args![value: "https://vector.dev"],
        want: Ok(value!({
                        "scheme": "https",
                        "username": "",
                        "password": "",
                        "host": "vector.dev",
                        "port": null,
                        "path": "/",
                        "query": {},
                        "fragment": null,
        }))
    }
}

bench_function! {
    parse_xml => vrl_stdlib::ParseXml;

    simple_text {
        args: func_args![ value: r#"<a>test</a>"# ],
        want: Ok(value!({ "a": "test" }))
    }

    include_attr {
        args: func_args![ value: r#"<a href="https://vector.dev">test</a>"# ],
        want: Ok(value!({ "a": { "@href": "https://vector.dev", "text": "test" } }))
    }

    exclude_attr {
        args: func_args![ value: r#"<a href="https://vector.dev">test</a>"#, include_attr: false ],
        want: Ok(value!({ "a": "test" }))
    }

    custom_text_key {
        args: func_args![ value: r#"<b>test</b>"#, text_key: "node", always_use_text_key: true ],
        want: Ok(value!({ "b": { "node": "test" } }))
    }

    nested_object {
        args: func_args![ value: r#"<a><b>one</b><c>two</c></a>"# ],
        want: Ok(value!({ "a": { "b": "one", "c": "two" } }))
    }

    nested_object_array {
        args: func_args![ value: r#"<a><b>one</b><b>two</b></a>"# ],
        want: Ok(value!({ "a": { "b": ["one", "two"] } }))
    }

    header_and_comments {
        args: func_args![ value: indoc!{r#"
            <?xml version="1.0" encoding="ISO-8859-1"?>
            <!-- Example found somewhere in the deep depths of the web -->
            <note>
                <to>Tove</to>
                <!-- Randomly inserted inner comment -->
                <from>Jani</from>
                <heading>Reminder</heading>
                <body>Don't forget me this weekend!</body>
            </note>

            <!-- Could literally be placed anywhere -->
        "#}],
        want: Ok(value!(
            {
                "note": {
                    "to": "Tove",
                    "from": "Jani",
                    "heading": "Reminder",
                    "body": "Don't forget me this weekend!"
                }
            }
        ))
    }

    mixed_types {
        args: func_args![ value: indoc!{r#"
            <?xml version="1.0" encoding="ISO-8859-1"?>
            <!-- Mixed types -->
            <data>
                <!-- Booleans -->
                <item>true</item>
                <item>false</item>
                <!-- String -->
                <item>string!</item>
                <!-- Empty object -->
                <item />
                <!-- Literal value "null" -->
                <item>null</item>
                <!-- Integer -->
                <item>1</item>
                <!-- Float -->
                <item>1.0</item>
            </data>
        "#}],
        want: Ok(value!(
            {
                "data": {
                    "item": [
                        true,
                        false,
                        "string!",
                        {},
                        null,
                        1,
                        1.0
                    ]
                }
            }
        ))
    }

    just_strings {
        args: func_args![ value: indoc!{r#"
            <?xml version="1.0" encoding="ISO-8859-1"?>
            <!-- All scalar types are just strings -->
            <data>
                <item>true</item>
                <item>false</item>
                <item>string!</item>
                <!-- Still an empty object -->
                <item />
                <item>null</item>
                <item>1</item>
                <item>1.0</item>
            </data>
        "#}, parse_null: false, parse_bool: false, parse_number: false],
        want: Ok(value!(
            {
                "data": {
                    "item": [
                        "true",
                        "false",
                        "string!",
                        {},
                        "null",
                        "1",
                        "1.0"
                    ]
                }
            }
        ))
    }

    untrimmed {
        args: func_args![ value: "<root>  <a>test</a>  </root>", trim: false ],
        want: Ok(value!(
            {
                "root": {
                    "a": "test",
                    "text": ["  ", "  "],
                }
            }
        ))
    }

    invalid_token {
        args: func_args![ value: "true" ],
        want: Err("unable to parse xml: unknown token at 1:1")
    }
}

bench_function! {
    push => vrl_stdlib::Push;

    literal {
        args: func_args![value: value!([11, false, 42.5]), item: "foo"],
        want: Ok(value!([11, false, 42.5, "foo"])),
    }
}

bench_function! {
    redact => vrl_stdlib::Redact;

    regex {
        args: func_args![
            value: "hello 123456 world",
            filters: vec![Regex::new(r"\d+").unwrap()],
        ],
        want: Ok("hello [REDACTED] world"),
    }

    us_social_security_number {
        args: func_args![
            value: "hello 123-12-1234 world",
            filters: vec!["us_social_security_number"],
        ],
        want: Ok("hello [REDACTED] world"),
    }
}

bench_function! {
    replace => vrl_stdlib::Replace;

    string {
        args: func_args![
            value: "I like apples and bananas",
            pattern: "a",
            with: "o",
        ],
        want: Ok("I like opples ond bononos")
    }

    regex {
        args: func_args![
            value: "I like apples and bananas",
            pattern: Regex::new("[a]").unwrap(),
            with: "o",
        ],
        want: Ok("I like opples ond bononos")
    }
}

bench_function! {
    round => vrl_stdlib::Round;

    integer {
        args: func_args![value: 1234.56789, precision: 4],
        want: Ok(1234.5679)
    }

    float {
        args: func_args![value: 1234, precision: 4],
        want: Ok(1234)
    }
}

bench_function! {
    sha1 => vrl_stdlib::Sha1;

    literal {
        args: func_args![value: "foo"],
        want: Ok("0beec7b5ea3f0fdbc95d0dd47f3c5bc275da8a33")
    }
}

bench_function! {
    sha2 => vrl_stdlib::Sha2;

    default {
        args: func_args![value: "foo"],
        want: Ok("d58042e6aa5a335e03ad576c6a9e43b41591bfd2077f72dec9df7930e492055d")
    }
}

bench_function! {
    sha3 => vrl_stdlib::Sha3;

    default {
        args: func_args![value: "foo"],
        want: Ok("4bca2b137edc580fe50a88983ef860ebaca36c857b1f492839d6d7392452a63c82cbebc68e3b70a2a1480b4bb5d437a7cba6ecf9d89f9ff3ccd14cd6146ea7e7")
    }
}

bench_function! {
    slice => vrl_stdlib::Slice;

    literal {
        args: func_args![
            value: "Supercalifragilisticexpialidocious",
            start: 5,
            end: 9,
        ],
        want: Ok("cali")
    }
}

bench_function! {
    split => vrl_stdlib::Split;

    string {
        args: func_args![value: "foo,bar,baz", pattern: ","],
        want: Ok(value!(["foo", "bar", "baz"]))
    }

    regex {
        args: func_args![value: "foo,bar,baz", pattern: Regex::new("[,]").unwrap()],
        want: Ok(value!(["foo", "bar", "baz"]))
    }
}

bench_function! {
    starts_with  => vrl_stdlib::StartsWith;

    case_sensitive {
        args: func_args![value: "abcdefg", substring: "abc", case_sensitive: true],
        want: Ok(value!(true)),
    }

    case_insensitive {
        args: func_args![value: "abcdefg", substring: "ABC", case_sensitive: false],
        want: Ok(value!(true)),
    }
}

bench_function! {
    strip_ansi_escape_codes => vrl_stdlib::StripAnsiEscapeCodes;

    literal {
        args: func_args![value: "\x1b[46mfoo\x1b[0m bar"],
        want: Ok("foo bar")
    }
}

bench_function! {
    strip_whitespace => vrl_stdlib::StripWhitespace;

    literal {
        args: func_args![
            value:" \u{3000}\u{205F}\u{202F}\u{A0}\u{9} ❤❤ hi there ❤❤  \u{9}\u{A0}\u{202F}\u{205F}\u{3000}"
        ],
        want: Ok("❤❤ hi there ❤❤")
    }
}

bench_function! {
    tag_types_externally => vrl_stdlib::TagTypesExternally;

    tag_bytes {
        args: func_args![value: "foo"],
        want: Ok(btreemap! {
            "string" => "foo",
        }),
    }

    tag_integer {
        args: func_args![value: 123],
        want: Ok(btreemap! {
            "integer" => 123
        }),
    }

    tag_float {
        args: func_args![value: 123.45],
        want: Ok(btreemap! {
            "float" => 123.45
        }),
    }

    tag_boolean {
        args: func_args![value: true],
        want: Ok(btreemap! {
            "boolean" => true
        }),
    }

    tag_map {
        args: func_args![value: btreemap! {"foo" => "bar"}],
        want: Ok(btreemap! {
            "foo" => btreemap! {
                "string" => "bar"
            }
        }),
    }

    tag_array {
        args: func_args![value: vec!["foo"]],
        want: Ok(vec![
            btreemap! {
                "string" => "foo"
            },
        ]),
    }

    tag_timestamp {
        args: func_args![value: Utc.ymd(2021, 1, 1).and_hms_milli(0, 0, 0, 0)],
        want: Ok(btreemap! {
            "timestamp" => Utc.ymd(2021, 1, 1).and_hms_milli(0, 0, 0, 0)
        }),
    }

    tag_regex {
        args: func_args![value: Regex::new(".*").unwrap()],
        want: Ok(btreemap! {
            "regex" => Regex::new(".*").unwrap()
        }),
    }

    tag_null {
        args: func_args![value: Value::Null],
        want: Ok(Value::Null),
    }
}

bench_function! {
    to_bool => vrl_stdlib::ToBool;

    string {
        args: func_args![value: "true"],
        want: Ok(true)
    }

    r#bool {
        args: func_args![value: true],
        want: Ok(true)
    }

    int {
        args: func_args![value: 20],
        want: Ok(true)
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok(false)
    }
}

bench_function! {
    to_float => vrl_stdlib::ToFloat;

    string {
        args: func_args![value: "2.0"],
        want: Ok(2.0)
    }

    r#bool {
        args: func_args![value: true],
        want: Ok(1.0)
    }

    float {
        args: func_args![value: 1.0],
        want: Ok(1.0)
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok(0.0)
    }
}

bench_function! {
    to_int => vrl_stdlib::ToInt;

    string {
        args: func_args![value: "2"],
        want: Ok(2)
    }

    r#bool {
        args: func_args![value: true],
        want: Ok(1)
    }

    int {
        args: func_args![value: 1],
        want: Ok(1)
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok(0)
    }
}

bench_function! {
    to_regex => vrl_stdlib::ToRegex;

    regex {
        args: func_args![value: "^foo.*bar.*baz"],
        want: Ok(Regex::new("^foo.*bar.*baz").expect("regex is valid"))
    }
}

bench_function! {
    to_string => vrl_stdlib::ToString;

    string {
        args: func_args![value: "2"],
        want: Ok("2")
    }

    r#bool {
        args: func_args![value: true],
        want: Ok("true")
    }

    int {
        args: func_args![value: 1],
        want: Ok("1")
    }

    null {
        args: func_args![value: value!(null)],
        want: Ok("")
    }
}

bench_function! {
    to_syslog_facility => vrl_stdlib::ToSyslogFacility;

    literal {
        args: func_args![value: value!(23)],
        want: Ok(value!("local7")),
    }
}

bench_function! {
    to_syslog_level => vrl_stdlib::ToSyslogLevel;

    literal {
        args: func_args![value: value!(5)],
        want: Ok(value!("notice")),
    }
}

bench_function! {
    to_syslog_severity => vrl_stdlib::ToSyslogSeverity;

    literal {
        args: func_args![value: value!("info")],
        want: Ok(value!(6)),
    }
}

bench_function! {
    to_timestamp => vrl_stdlib::ToTimestamp;

    string {
        args: func_args![value: "2001-07-08T00:34:60.026490+09:30"],
        want: Ok(DateTime::parse_from_rfc3339("2001-07-08T00:34:60.026490+09:30").unwrap().with_timezone(&Utc))
    }

    int {
        args: func_args![value: 1612814266],
        want: Ok(DateTime::parse_from_rfc3339("2021-02-08T19:57:46+00:00").unwrap().with_timezone(&Utc))
    }

    float {
        args: func_args![value: 1612814266.1],
        want: Ok(DateTime::parse_from_rfc3339("2021-02-08T19:57:46.099999905+00:00").unwrap().with_timezone(&Utc))
    }
}

bench_function! {
    to_unix_timestamp => vrl_stdlib::ToUnixTimestamp;

    default {
        args: func_args![value: Utc.ymd(2021, 1, 1).and_hms_milli(0, 0, 0, 0)],
        want: Ok(1609459200),
    }
}

bench_function! {
    truncate => vrl_stdlib::Truncate;

    ellipsis {
        args: func_args![
            value: "Supercalifragilisticexpialidocious",
            limit: 5,
            ellipsis: true,
        ],
        want: Ok("Super..."),
    }

    no_ellipsis {
        args: func_args![
            value: "Supercalifragilisticexpialidocious",
            limit: 5,
            ellipsis: false,
        ],
        want: Ok("Super"),
    }
}

bench_function! {
    upcase => vrl_stdlib::Upcase;

    literal {
        args: func_args![value: "foo"],
        want: Ok("FOO")
    }
}
