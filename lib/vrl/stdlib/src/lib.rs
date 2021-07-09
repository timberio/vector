mod util;

#[cfg(feature = "append")]
mod append;
#[cfg(feature = "array")]
mod array;
#[cfg(feature = "assert")]
mod assert;
#[cfg(feature = "assert_eq")]
mod assert_eq;
#[cfg(feature = "boolean")]
mod boolean;
#[cfg(feature = "ceil")]
mod ceil;
#[cfg(feature = "compact")]
mod compact;
#[cfg(feature = "contains")]
mod contains;
#[cfg(feature = "decode_base64")]
mod decode_base64;
#[cfg(feature = "decode_percent")]
mod decode_percent;
#[cfg(feature = "del")]
mod del;
#[cfg(feature = "downcase")]
mod downcase;
#[cfg(feature = "encode_base64")]
mod encode_base64;
#[cfg(feature = "encode_json")]
mod encode_json;
#[cfg(feature = "encode_key_value")]
mod encode_key_value;
#[cfg(feature = "encode_logfmt")]
mod encode_logfmt;
#[cfg(feature = "encode_percent")]
mod encode_percent;
#[cfg(feature = "ends_with")]
mod ends_with;
#[cfg(feature = "exists")]
mod exists;
#[cfg(feature = "flatten")]
mod flatten;
#[cfg(feature = "float")]
mod float;
#[cfg(feature = "floor")]
mod floor;
#[cfg(feature = "format_int")]
mod format_int;
#[cfg(feature = "format_number")]
mod format_number;
#[cfg(feature = "format_timestamp")]
mod format_timestamp;
#[cfg(feature = "get_env_var")]
mod get_env_var;
#[cfg(feature = "get_hostname")]
mod get_hostname;
#[cfg(feature = "includes")]
mod includes;
#[cfg(feature = "integer")]
mod integer;
#[cfg(feature = "ip_aton")]
mod ip_aton;
#[cfg(feature = "ip_cidr_contains")]
mod ip_cidr_contains;
#[cfg(feature = "ip_ntoa")]
mod ip_ntoa;
#[cfg(feature = "ip_subnet")]
mod ip_subnet;
#[cfg(feature = "ip_to_ipv6")]
mod ip_to_ipv6;
#[cfg(feature = "ipv6_to_ipv4")]
mod ipv6_to_ipv4;
#[cfg(feature = "is_array")]
mod is_array;
#[cfg(feature = "is_boolean")]
mod is_boolean;
#[cfg(feature = "is_float")]
mod is_float;
#[cfg(feature = "is_integer")]
mod is_integer;
#[cfg(feature = "is_null")]
mod is_null;
#[cfg(feature = "is_nullish")]
mod is_nullish;
#[cfg(feature = "is_object")]
mod is_object;
#[cfg(feature = "is_regex")]
mod is_regex;
#[cfg(feature = "is_string")]
mod is_string;
#[cfg(feature = "is_timestamp")]
mod is_timestamp;
#[cfg(feature = "join")]
mod join;
#[cfg(feature = "length")]
mod length;
#[cfg(feature = "log")]
mod log;
#[cfg(any(
    feature = "parse_common_log",
    feature = "parse_apache_log",
    feature = "parse_nginx_log"
))]
mod log_util;
#[cfg(feature = "match")]
mod r#match;
#[cfg(feature = "match_any")]
mod match_any;
#[cfg(feature = "match_array")]
mod match_array;
#[cfg(feature = "md5")]
mod md5;
#[cfg(feature = "merge")]
mod merge;
#[cfg(feature = "now")]
mod now;
#[cfg(feature = "object")]
mod object;
#[cfg(feature = "only_fields")]
mod only_fields;
#[cfg(feature = "parse_apache_log")]
mod parse_apache_log;
#[cfg(feature = "parse_aws_alb_log")]
mod parse_aws_alb_log;
#[cfg(feature = "parse_aws_cloudwatch_log_subscription_message")]
mod parse_aws_cloudwatch_log_subscription_message;
#[cfg(feature = "parse_aws_vpc_flow_log")]
mod parse_aws_vpc_flow_log;
#[cfg(feature = "parse_common_log")]
mod parse_common_log;
#[cfg(feature = "parse_csv")]
mod parse_csv;
#[cfg(feature = "parse_datadog_grok")]
mod parse_datadog_grok;
#[cfg(feature = "parse_duration")]
mod parse_duration;
#[cfg(feature = "parse_glog")]
mod parse_glog;
#[cfg(feature = "parse_grok")]
mod parse_grok;
#[cfg(feature = "parse_int")]
mod parse_int;
#[cfg(feature = "parse_json")]
mod parse_json;
#[cfg(feature = "parse_key_value")]
mod parse_key_value;
#[cfg(feature = "parse_klog")]
mod parse_klog;
#[cfg(feature = "parse_linux_authorization")]
mod parse_linux_authorization;
#[cfg(feature = "parse_logfmt")]
mod parse_logfmt;
#[cfg(feature = "parse_nginx_log")]
mod parse_nginx_log;
#[cfg(feature = "parse_query_string")]
mod parse_query_string;
#[cfg(feature = "parse_regex")]
mod parse_regex;
#[cfg(feature = "parse_regex_all")]
mod parse_regex_all;
#[cfg(feature = "parse_ruby_hash")]
mod parse_ruby_hash;
#[cfg(feature = "parse_syslog")]
mod parse_syslog;
#[cfg(feature = "parse_timestamp")]
mod parse_timestamp;
#[cfg(feature = "parse_tokens")]
mod parse_tokens;
#[cfg(feature = "parse_url")]
mod parse_url;
#[cfg(feature = "parse_xml")]
mod parse_xml;
#[cfg(feature = "push")]
mod push;
#[cfg(feature = "redact")]
mod redact;
#[cfg(feature = "replace")]
mod replace;
#[cfg(feature = "round")]
mod round;
#[cfg(feature = "sha1")]
mod sha1;
#[cfg(feature = "sha2")]
mod sha2;
#[cfg(feature = "sha3")]
mod sha3;
#[cfg(feature = "slice")]
mod slice;
#[cfg(feature = "split")]
mod split;
#[cfg(feature = "starts_with")]
mod starts_with;
#[cfg(feature = "string")]
mod string;
#[cfg(feature = "strip_ansi_escape_codes")]
mod strip_ansi_escape_codes;
#[cfg(feature = "strip_whitespace")]
mod strip_whitespace;
#[cfg(feature = "tag_types_externally")]
mod tag_types_externally;
#[cfg(feature = "timestamp")]
mod timestamp;
#[cfg(feature = "to_bool")]
mod to_bool;
#[cfg(feature = "to_float")]
mod to_float;
#[cfg(feature = "to_int")]
mod to_int;
#[cfg(feature = "to_regex")]
mod to_regex;
#[cfg(feature = "to_string")]
mod to_string;
#[cfg(feature = "to_syslog_facility")]
mod to_syslog_facility;
#[cfg(feature = "to_syslog_level")]
mod to_syslog_level;
#[cfg(feature = "to_syslog_severity")]
mod to_syslog_severity;
#[cfg(feature = "to_timestamp")]
mod to_timestamp;
#[cfg(feature = "to_unix_timestamp")]
mod to_unix_timestamp;
#[cfg(feature = "truncate")]
mod truncate;
#[cfg(feature = "upcase")]
mod upcase;
#[cfg(feature = "uuid_v4")]
mod uuid_v4;

// -----------------------------------------------------------------------------

#[cfg(feature = "array")]
pub use crate::array::Array;
#[cfg(feature = "md5")]
pub use crate::md5::Md5;
#[cfg(feature = "parse_datadog_grok")]
pub use crate::parse_datadog_grok::ParseDatadogGrok;
#[cfg(feature = "sha1")]
pub use crate::sha1::Sha1;
#[cfg(feature = "append")]
pub use append::Append;
#[cfg(feature = "assert")]
pub use assert::Assert;
#[cfg(feature = "assert_eq")]
pub use assert_eq::AssertEq;
#[cfg(feature = "boolean")]
pub use boolean::Boolean;
#[cfg(feature = "ceil")]
pub use ceil::Ceil;
#[cfg(feature = "compact")]
pub use compact::Compact;
#[cfg(feature = "contains")]
pub use contains::Contains;
#[cfg(feature = "decode_base64")]
pub use decode_base64::DecodeBase64;
#[cfg(feature = "decode_percent")]
pub use decode_percent::DecodePercent;
#[cfg(feature = "del")]
pub use del::Del;
#[cfg(feature = "downcase")]
pub use downcase::Downcase;
#[cfg(feature = "encode_base64")]
pub use encode_base64::EncodeBase64;
#[cfg(feature = "encode_json")]
pub use encode_json::EncodeJson;
#[cfg(feature = "encode_key_value")]
pub use encode_key_value::EncodeKeyValue;
#[cfg(feature = "encode_logfmt")]
pub use encode_logfmt::EncodeLogfmt;
#[cfg(feature = "encode_percent")]
pub use encode_percent::EncodePercent;
#[cfg(feature = "ends_with")]
pub use ends_with::EndsWith;
#[cfg(feature = "exists")]
pub use exists::Exists;
#[cfg(feature = "flatten")]
pub use flatten::Flatten;
#[cfg(feature = "float")]
pub use float::Float;
#[cfg(feature = "floor")]
pub use floor::Floor;
#[cfg(feature = "format_int")]
pub use format_int::FormatInt;
#[cfg(feature = "format_number")]
pub use format_number::FormatNumber;
#[cfg(feature = "format_timestamp")]
pub use format_timestamp::FormatTimestamp;
#[cfg(feature = "get_env_var")]
pub use get_env_var::GetEnvVar;
#[cfg(feature = "get_hostname")]
pub use get_hostname::GetHostname;
#[cfg(feature = "includes")]
pub use includes::Includes;
#[cfg(feature = "integer")]
pub use integer::Integer;
#[cfg(feature = "ip_aton")]
pub use ip_aton::IpAton;
#[cfg(feature = "ip_cidr_contains")]
pub use ip_cidr_contains::IpCidrContains;
#[cfg(feature = "ip_ntoa")]
pub use ip_ntoa::IpNtoa;
#[cfg(feature = "ip_subnet")]
pub use ip_subnet::IpSubnet;
#[cfg(feature = "ip_to_ipv6")]
pub use ip_to_ipv6::IpToIpv6;
#[cfg(feature = "ipv6_to_ipv4")]
pub use ipv6_to_ipv4::Ipv6ToIpV4;
#[cfg(feature = "is_array")]
pub use is_array::IsArray;
#[cfg(feature = "is_boolean")]
pub use is_boolean::IsBoolean;
#[cfg(feature = "is_float")]
pub use is_float::IsFloat;
#[cfg(feature = "is_integer")]
pub use is_integer::IsInteger;
#[cfg(feature = "is_null")]
pub use is_null::IsNull;
#[cfg(feature = "is_nullish")]
pub use is_nullish::IsNullish;
#[cfg(feature = "is_object")]
pub use is_object::IsObject;
#[cfg(feature = "is_regex")]
pub use is_regex::IsRegex;
#[cfg(feature = "is_string")]
pub use is_string::IsString;
#[cfg(feature = "is_timestamp")]
pub use is_timestamp::IsTimestamp;
#[cfg(feature = "join")]
pub use join::Join;
#[cfg(feature = "length")]
pub use length::Length;
#[cfg(feature = "log")]
pub use log::Log;
#[cfg(feature = "match_any")]
pub use match_any::MatchAny;
#[cfg(feature = "match_array")]
pub use match_array::MatchArray;
#[cfg(feature = "merge")]
pub use merge::Merge;
#[cfg(feature = "now")]
pub use now::Now;
#[cfg(feature = "object")]
pub use object::Object;
#[cfg(feature = "only_fields")]
pub use only_fields::OnlyFields;
#[cfg(feature = "parse_apache_log")]
pub use parse_apache_log::ParseApacheLog;
#[cfg(feature = "parse_aws_alb_log")]
pub use parse_aws_alb_log::ParseAwsAlbLog;
#[cfg(feature = "parse_aws_cloudwatch_log_subscription_message")]
pub use parse_aws_cloudwatch_log_subscription_message::ParseAwsCloudWatchLogSubscriptionMessage;
#[cfg(feature = "parse_aws_vpc_flow_log")]
pub use parse_aws_vpc_flow_log::ParseAwsVpcFlowLog;
#[cfg(feature = "parse_common_log")]
pub use parse_common_log::ParseCommonLog;
#[cfg(feature = "parse_csv")]
pub use parse_csv::ParseCsv;
#[cfg(feature = "parse_duration")]
pub use parse_duration::ParseDuration;
#[cfg(feature = "parse_glog")]
pub use parse_glog::ParseGlog;
#[cfg(feature = "parse_grok")]
pub use parse_grok::ParseGrok;
#[cfg(feature = "parse_int")]
pub use parse_int::ParseInt;
#[cfg(feature = "parse_json")]
pub use parse_json::ParseJson;
#[cfg(feature = "parse_key_value")]
pub use parse_key_value::ParseKeyValue;
#[cfg(feature = "parse_klog")]
pub use parse_klog::ParseKlog;
#[cfg(feature = "parse_linux_authorization")]
pub use parse_linux_authorization::ParseLinuxAuthorization;
#[cfg(feature = "parse_logfmt")]
pub use parse_logfmt::ParseLogFmt;
#[cfg(feature = "parse_nginx_log")]
pub use parse_nginx_log::ParseNginxLog;
#[cfg(feature = "parse_query_string")]
pub use parse_query_string::ParseQueryString;
#[cfg(feature = "parse_regex")]
pub use parse_regex::ParseRegex;
#[cfg(feature = "parse_regex_all")]
pub use parse_regex_all::ParseRegexAll;
#[cfg(feature = "parse_ruby_hash")]
pub use parse_ruby_hash::ParseRubyHash;
#[cfg(feature = "parse_syslog")]
pub use parse_syslog::ParseSyslog;
#[cfg(feature = "parse_timestamp")]
pub use parse_timestamp::ParseTimestamp;
#[cfg(feature = "parse_tokens")]
pub use parse_tokens::ParseTokens;
#[cfg(feature = "parse_url")]
pub use parse_url::ParseUrl;
#[cfg(feature = "parse_xml")]
pub use parse_xml::ParseXml;
#[cfg(feature = "push")]
pub use push::Push;
#[cfg(feature = "match")]
pub use r#match::Match;
#[cfg(feature = "redact")]
pub use redact::Redact;
#[cfg(feature = "replace")]
pub use replace::Replace;
#[cfg(feature = "round")]
pub use round::Round;
#[cfg(feature = "sha2")]
pub use sha2::Sha2;
#[cfg(feature = "sha3")]
pub use sha3::Sha3;
#[cfg(feature = "slice")]
pub use slice::Slice;
#[cfg(feature = "split")]
pub use split::Split;
#[cfg(feature = "starts_with")]
pub use starts_with::StartsWith;
#[cfg(feature = "string")]
pub use string::String;
#[cfg(feature = "strip_ansi_escape_codes")]
pub use strip_ansi_escape_codes::StripAnsiEscapeCodes;
#[cfg(feature = "strip_whitespace")]
pub use strip_whitespace::StripWhitespace;
#[cfg(feature = "tag_types_externally")]
pub use tag_types_externally::TagTypesExternally;
#[cfg(feature = "timestamp")]
pub use timestamp::Timestamp;
#[cfg(feature = "to_bool")]
pub use to_bool::ToBool;
#[cfg(feature = "to_float")]
pub use to_float::ToFloat;
#[cfg(feature = "to_int")]
pub use to_int::ToInt;
#[cfg(feature = "to_regex")]
pub use to_regex::ToRegex;
#[cfg(feature = "to_string")]
pub use to_string::ToString;
#[cfg(feature = "to_syslog_facility")]
pub use to_syslog_facility::ToSyslogFacility;
#[cfg(feature = "to_syslog_level")]
pub use to_syslog_level::ToSyslogLevel;
#[cfg(feature = "to_syslog_severity")]
pub use to_syslog_severity::ToSyslogSeverity;
#[cfg(feature = "to_timestamp")]
pub use to_timestamp::ToTimestamp;
#[cfg(feature = "to_unix_timestamp")]
pub use to_unix_timestamp::ToUnixTimestamp;
#[cfg(feature = "truncate")]
pub use truncate::Truncate;
#[cfg(feature = "upcase")]
pub use upcase::Upcase;
#[cfg(feature = "uuid_v4")]
pub use uuid_v4::UuidV4;

pub fn all() -> Vec<Box<dyn vrl::Function>> {
    vec![
        #[cfg(feature = "array")]
        Box::new(Array),
        #[cfg(feature = "append")]
        Box::new(Append),
        #[cfg(feature = "assert")]
        Box::new(Assert),
        #[cfg(feature = "assert_eq")]
        Box::new(AssertEq),
        #[cfg(feature = "boolean")]
        Box::new(Boolean),
        #[cfg(feature = "ceil")]
        Box::new(Ceil),
        #[cfg(feature = "compact")]
        Box::new(Compact),
        #[cfg(feature = "contains")]
        Box::new(Contains),
        #[cfg(feature = "decode_base64")]
        Box::new(DecodeBase64),
        #[cfg(feature = "decode_percent")]
        Box::new(DecodePercent),
        #[cfg(feature = "del")]
        Box::new(Del),
        #[cfg(feature = "downcase")]
        Box::new(Downcase),
        #[cfg(feature = "encode_base64")]
        Box::new(EncodeBase64),
        #[cfg(feature = "encode_json")]
        Box::new(EncodeJson),
        #[cfg(feature = "encode_key_value")]
        Box::new(EncodeKeyValue),
        #[cfg(feature = "encode_logfmt")]
        Box::new(EncodeLogfmt),
        #[cfg(feature = "encode_percent")]
        Box::new(EncodePercent),
        #[cfg(feature = "ends_with")]
        Box::new(EndsWith),
        #[cfg(feature = "exists")]
        Box::new(Exists),
        #[cfg(feature = "flatten")]
        Box::new(Flatten),
        #[cfg(feature = "float")]
        Box::new(Float),
        #[cfg(feature = "floor")]
        Box::new(Floor),
        #[cfg(feature = "format_int")]
        Box::new(FormatInt),
        #[cfg(feature = "format_number")]
        Box::new(FormatNumber),
        #[cfg(feature = "format_timestamp")]
        Box::new(FormatTimestamp),
        #[cfg(feature = "get_env_var")]
        Box::new(GetEnvVar),
        #[cfg(feature = "get_hostname")]
        Box::new(GetHostname),
        #[cfg(feature = "includes")]
        Box::new(Includes),
        #[cfg(feature = "integer")]
        Box::new(Integer),
        #[cfg(feature = "ip_aton")]
        Box::new(IpAton),
        #[cfg(feature = "ip_cidr_contains")]
        Box::new(IpCidrContains),
        #[cfg(feature = "ip_ntoa")]
        Box::new(IpNtoa),
        #[cfg(feature = "ip_subnet")]
        Box::new(IpSubnet),
        #[cfg(feature = "ip_to_ipv6")]
        Box::new(IpToIpv6),
        #[cfg(feature = "ipv6_to_ipv4")]
        Box::new(Ipv6ToIpV4),
        #[cfg(feature = "is_array")]
        Box::new(IsArray),
        #[cfg(feature = "is_boolean")]
        Box::new(IsBoolean),
        #[cfg(feature = "is_float")]
        Box::new(IsFloat),
        #[cfg(feature = "is_integer")]
        Box::new(IsInteger),
        #[cfg(feature = "is_null")]
        Box::new(IsNull),
        #[cfg(feature = "is_nullish")]
        Box::new(IsNullish),
        #[cfg(feature = "is_object")]
        Box::new(IsObject),
        #[cfg(feature = "is_regex")]
        Box::new(IsRegex),
        #[cfg(feature = "is_string")]
        Box::new(IsString),
        #[cfg(feature = "is_timestamp")]
        Box::new(IsTimestamp),
        #[cfg(feature = "join")]
        Box::new(Join),
        #[cfg(feature = "length")]
        Box::new(Length),
        #[cfg(feature = "log")]
        Box::new(Log),
        #[cfg(feature = "match")]
        Box::new(Match),
        #[cfg(feature = "match_any")]
        Box::new(MatchAny),
        #[cfg(feature = "match_array")]
        Box::new(MatchArray),
        #[cfg(feature = "md5")]
        Box::new(Md5),
        #[cfg(feature = "merge")]
        Box::new(Merge),
        #[cfg(feature = "now")]
        Box::new(Now),
        // We are not sure if this is the way we want to expose this functionality yet
        // https://github.com/timberio/vector/issues/5607
        //#[cfg(feature = "only_fields")]
        //Box::new(OnlyFields),
        #[cfg(feature = "object")]
        Box::new(Object),
        #[cfg(feature = "parse_aws_alb_log")]
        Box::new(ParseAwsAlbLog),
        #[cfg(feature = "parse_aws_cloudwatch_log_subscription_message")]
        Box::new(ParseAwsCloudWatchLogSubscriptionMessage),
        #[cfg(feature = "parse_aws_vpc_flow_log")]
        Box::new(ParseAwsVpcFlowLog),
        #[cfg(feature = "parse_duration")]
        Box::new(ParseDuration),
        #[cfg(feature = "parse_glog")]
        Box::new(ParseGlog),
        #[cfg(feature = "parse_datadog_grok")]
        Box::new(ParseDatadogGrok),
        #[cfg(feature = "parse_grok")]
        Box::new(ParseGrok),
        #[cfg(feature = "parse_int")]
        Box::new(ParseInt),
        #[cfg(feature = "parse_json")]
        Box::new(ParseJson),
        #[cfg(feature = "parse_apache_log")]
        Box::new(ParseApacheLog),
        #[cfg(feature = "parse_common_log")]
        Box::new(ParseCommonLog),
        #[cfg(feature = "parse_csv")]
        Box::new(ParseCsv),
        #[cfg(feature = "parse_key_value")]
        Box::new(ParseKeyValue),
        #[cfg(feature = "parse_klog")]
        Box::new(ParseKlog),
        #[cfg(feature = "parse_linux_authorization")]
        Box::new(ParseLinuxAuthorization),
        #[cfg(feature = "parse_logfmt")]
        Box::new(ParseLogFmt),
        #[cfg(feature = "parse_nginx_log")]
        Box::new(ParseNginxLog),
        #[cfg(feature = "parse_query_string")]
        Box::new(ParseQueryString),
        #[cfg(feature = "parse_regex")]
        Box::new(ParseRegex),
        #[cfg(feature = "parse_regex_all")]
        Box::new(ParseRegexAll),
        #[cfg(feature = "parse_ruby_hash")]
        Box::new(ParseRubyHash),
        #[cfg(feature = "parse_syslog")]
        Box::new(ParseSyslog),
        #[cfg(feature = "parse_timestamp")]
        Box::new(ParseTimestamp),
        #[cfg(feature = "parse_tokens")]
        Box::new(ParseTokens),
        #[cfg(feature = "parse_url")]
        Box::new(ParseUrl),
        #[cfg(feature = "parse_xml")]
        Box::new(ParseXml),
        #[cfg(feature = "push")]
        Box::new(Push),
        #[cfg(feature = "match")]
        Box::new(Match),
        #[cfg(feature = "match_any")]
        Box::new(MatchAny),
        #[cfg(feature = "match_array")]
        Box::new(MatchArray),
        #[cfg(feature = "redact")]
        Box::new(Redact),
        #[cfg(feature = "replace")]
        Box::new(Replace),
        #[cfg(feature = "round")]
        Box::new(Round),
        #[cfg(feature = "sha1")]
        Box::new(Sha1),
        #[cfg(feature = "sha2")]
        Box::new(Sha2),
        #[cfg(feature = "sha3")]
        Box::new(Sha3),
        #[cfg(feature = "slice")]
        Box::new(Slice),
        #[cfg(feature = "split")]
        Box::new(Split),
        #[cfg(feature = "starts_with")]
        Box::new(StartsWith),
        #[cfg(feature = "string")]
        Box::new(String),
        #[cfg(feature = "strip_ansi_escape_codes")]
        Box::new(StripAnsiEscapeCodes),
        #[cfg(feature = "strip_whitespace")]
        Box::new(StripWhitespace),
        #[cfg(feature = "tag_types_externally")]
        Box::new(TagTypesExternally),
        #[cfg(feature = "timestamp")]
        Box::new(Timestamp),
        #[cfg(feature = "to_bool")]
        Box::new(ToBool),
        #[cfg(feature = "to_float")]
        Box::new(ToFloat),
        #[cfg(feature = "to_int")]
        Box::new(ToInt),
        #[cfg(feature = "to_regex")]
        Box::new(ToRegex),
        #[cfg(feature = "to_string")]
        Box::new(ToString),
        #[cfg(feature = "to_syslog_facility")]
        Box::new(ToSyslogFacility),
        #[cfg(feature = "to_syslog_level")]
        Box::new(ToSyslogLevel),
        #[cfg(feature = "to_syslog_severity")]
        Box::new(ToSyslogSeverity),
        #[cfg(feature = "to_timestamp")]
        Box::new(ToTimestamp),
        #[cfg(feature = "to_unix_timestamp")]
        Box::new(ToUnixTimestamp),
        #[cfg(feature = "truncate")]
        Box::new(Truncate),
        #[cfg(feature = "upcase")]
        Box::new(Upcase),
        #[cfg(feature = "uuid_v4")]
        Box::new(UuidV4),
    ]
}
