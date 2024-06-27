use std::str::FromStr;

use once_cell::sync::Lazy;
use strum_macros::EnumIter;

use crate::cachestr::Cachestr;

use super::misc::hash16;

macro_rules! http_field {
    ($name:ident,$s:expr) => {
        static $name: Lazy<(Cachestr, u16)> =
            Lazy::new(|| (Cachestr::from($s), hash16($s.as_bytes())));
    };
}

http_field!(ACCEPT, "Accept");
http_field!(ACCEPT_ENCODING, "Accept-Encoding");
http_field!(ACCEPT_LANGUAGE, "Accept-Language");
http_field!(ACCEPT_PATCH, "Accept-Patch");
http_field!(ACCEPT_POST, "Accept-Post");
http_field!(ACCEPT_RANGES, "Accept-Ranges");
http_field!(
    ACCESS_CONTROL_ALLOW_CREDENTIALS,
    "Access-Control-Allow-Credentials"
);
http_field!(ACCESS_CONTROL_ALLOW_HEADERS, "Access-Control-Allow-Headers");
http_field!(ACCESS_CONTROL_ALLOW_METHODS, "Access-Control-Allow-Methods");
http_field!(ACCESS_CONTROL_ALLOW_ORIGIN, "Access-Control-Allow-Origin");
http_field!(
    ACCESS_CONTROL_EXPOSE_HEADERS,
    "Access-Control-Expose-Headers"
);
http_field!(ACCESS_CONTROL_MAX_AGE, "Access-Control-Max-Age");
http_field!(
    ACCESS_CONTROL_REQUEST_HEADERS,
    "Access-Control-Request-Headers"
);
http_field!(
    ACCESS_CONTROL_REQUEST_METHOD,
    "Access-Control-Request-Method"
);
http_field!(AGE, "Age");
http_field!(ALLOW, "Allow");
http_field!(AUTHORIZATION, "Authorization");
http_field!(CACHE_CONTROL, "Cache-Control");
http_field!(CLEAR_SITE_DATA, "Clear-Site-Data");
http_field!(CONNECTION, "Connection");
http_field!(CONTENT_DISPOSITION, "Content-Disposition");
http_field!(CONTENT_ENCODING, "Content-Encoding");
http_field!(CONTENT_LANGUAGE, "Content-Language");
http_field!(CONTENT_LENGTH, "Content-Length");
http_field!(CONTENT_LOCATION, "Content-Location");
http_field!(CONTENT_RANGE, "Content-Range");
http_field!(CONTENT_SECURITY_POLICY, "Content-Security-Policy");
http_field!(
    CONTENT_SECURITY_POLICY_REPORT_ONLY,
    "Content-Security-Policy-Report-Only"
);
http_field!(CONTENT_TYPE, "Content-Type");
http_field!(COOKIE, "Cookie");
http_field!(CROSS_ORIGIN_EMBEDDER_POLICY, "Cross-Origin-Embedder-Policy");
http_field!(CROSS_ORIGIN_OPENER_POLICY, "Cross-Origin-Opener-Policy");
http_field!(CROSS_ORIGIN_RESOURCE_POLICY, "Cross-Origin-Resource-Policy");
http_field!(DATE, "Date");
http_field!(ETAG, "ETag");
http_field!(EXPECT, "Expect");
http_field!(EXPIRES, "Expires");
http_field!(FORWARDED, "Forwarded");
http_field!(HOST, "Host");
http_field!(IF_MATCH, "If-Match");
http_field!(IF_MODIFIED_SINCE, "If-Modified-Since");
http_field!(IF_NONE_MATCH, "If-None-Match");
http_field!(IF_RANGE, "If-Range");
http_field!(IF_UNMODIFIED_SINCE, "If-Unmodified-Since");
http_field!(KEEPALIVE, "Keep-Alive");
http_field!(LAST_MODIFIED, "Last-Modified");
http_field!(LINK, "Link");
http_field!(LOCATION, "Location");
http_field!(MAX_FORWARDS, "Max-Forwards");
http_field!(ORIGIN, "Origin");
http_field!(PROXY_AUTHENTICATE, "Proxy-Authenticate");
http_field!(PROXY_AUTHORIZATION, "Proxy-Authorization");
http_field!(RANGE, "Range");
http_field!(REFERER, "Referer");
http_field!(REFERRER_POLICY, "Referrer-Policy");
http_field!(RETRY_AFTER, "Retry-After");
http_field!(SERVER, "Server");
http_field!(SERVER_TIMING, "Server-Timing");
http_field!(SET_COOKIE, "Set-Cookie");
http_field!(SOURCE_MAP, "Source-Map");
http_field!(TE, "TE");
http_field!(TRAILER, "Trailer");
http_field!(TRANSFER_ENCODING, "Transfer-Encoding");
http_field!(UPGRADE, "Upgrade");
http_field!(USER_AGENT, "User-Agent");
http_field!(VARY, "Vary");
http_field!(VIA, "Via");
http_field!(WWW_AUTHENTICATE, "WWW-Authenticate");
http_field!(X_REAL_IP, "X-Real-Ip");
http_field!(X_FORWARDED_FOR, "X-Forwarded-For");

/// https://www.iana.org/assignments/http-fields/http-fields.xhtml
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, EnumIter)]
pub enum HttpField {
    Accept,
    AcceptEncoding,
    AcceptLanguage,
    AcceptPatch,
    AcceptPost,
    AcceptRanges,
    AccessControlAllowCredentials,
    AccessControlAllowHeaders,
    AccessControlAllowMethods,
    AccessControlAllowOrigin,
    AccessControlExposeHeaders,
    AccessControlMaxAge,
    AccessControlRequestHeaders,
    AccessControlRequestMethod,
    Age,
    Allow,
    Authorization,
    CacheControl,
    ClearSiteData,
    Connection,
    ContentDisposition,
    ContentEncoding,
    ContentLanguage,
    ContentLength,
    ContentLocation,
    ContentRange,
    ContentSecurityPolicy,
    ContentSecurityPolicyReportOnly,
    ContentType,
    Cookie,
    CrossOriginEmbedderPolicy,
    CrossOriginOpenerPolicy,
    CrossOriginResourcePolicy,
    Date,
    ETag,
    Expect,
    Expires,
    Forwarded,
    Host,
    IfMatch,
    IfModifiedSince,
    IfNoneMatch,
    IfRange,
    IfUnmodifiedSince,
    KeepAlive,
    LastModified,
    Link,
    Location,
    MaxForwards,
    Origin,
    ProxyAuthenticate,
    ProxyAuthorization,
    Range,
    Referer,
    ReferrerPolicy,
    RetryAfter,
    Server,
    ServerTiming,
    SetCookie,
    SourceMap,
    TE,
    Trailer,
    TransferEncoding,
    Upgrade,
    UserAgent,
    Vary,
    Via,
    WWWAuthenticate,
    XRealIp,
    XForwardedFor,
}

impl HttpField {
    #[inline(always)]
    fn get(&self) -> &(Cachestr, u16) {
        match self {
            HttpField::Accept => &ACCEPT,
            HttpField::AcceptEncoding => &ACCEPT_ENCODING,
            HttpField::AcceptLanguage => &ACCEPT_LANGUAGE,
            HttpField::AcceptPatch => &ACCEPT_PATCH,
            HttpField::AcceptPost => &ACCEPT_POST,
            HttpField::AcceptRanges => &ACCEPT_RANGES,
            HttpField::AccessControlAllowCredentials => &ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HttpField::AccessControlAllowHeaders => &ACCESS_CONTROL_ALLOW_HEADERS,
            HttpField::AccessControlAllowMethods => &ACCESS_CONTROL_ALLOW_METHODS,
            HttpField::AccessControlAllowOrigin => &ACCESS_CONTROL_ALLOW_ORIGIN,
            HttpField::AccessControlExposeHeaders => &ACCESS_CONTROL_EXPOSE_HEADERS,
            HttpField::AccessControlMaxAge => &ACCESS_CONTROL_MAX_AGE,
            HttpField::AccessControlRequestHeaders => &ACCESS_CONTROL_REQUEST_HEADERS,
            HttpField::AccessControlRequestMethod => &ACCESS_CONTROL_REQUEST_METHOD,
            HttpField::Age => &AGE,
            HttpField::Allow => &ALLOW,
            HttpField::Authorization => &AUTHORIZATION,
            HttpField::CacheControl => &CACHE_CONTROL,
            HttpField::ClearSiteData => &CLEAR_SITE_DATA,
            HttpField::Connection => &CONNECTION,
            HttpField::ContentDisposition => &CONTENT_DISPOSITION,
            HttpField::ContentEncoding => &CONTENT_ENCODING,
            HttpField::ContentLanguage => &CONTENT_LANGUAGE,
            HttpField::ContentLength => &CONTENT_LENGTH,
            HttpField::ContentLocation => &CONTENT_LOCATION,
            HttpField::ContentRange => &CONTENT_RANGE,
            HttpField::ContentSecurityPolicy => &CONTENT_SECURITY_POLICY,
            HttpField::ContentSecurityPolicyReportOnly => &CONTENT_SECURITY_POLICY_REPORT_ONLY,
            HttpField::ContentType => &CONTENT_TYPE,
            HttpField::Cookie => &COOKIE,
            HttpField::CrossOriginEmbedderPolicy => &CROSS_ORIGIN_EMBEDDER_POLICY,
            HttpField::CrossOriginOpenerPolicy => &CROSS_ORIGIN_OPENER_POLICY,
            HttpField::CrossOriginResourcePolicy => &CROSS_ORIGIN_RESOURCE_POLICY,
            HttpField::Date => &DATE,
            HttpField::ETag => &ETAG,
            HttpField::Expect => &EXPECT,
            HttpField::Expires => &EXPIRES,
            HttpField::Forwarded => &FORWARDED,
            HttpField::Host => &HOST,
            HttpField::IfMatch => &IF_MATCH,
            HttpField::IfModifiedSince => &IF_MODIFIED_SINCE,
            HttpField::IfNoneMatch => &IF_NONE_MATCH,
            HttpField::IfRange => &IF_RANGE,
            HttpField::IfUnmodifiedSince => &IF_UNMODIFIED_SINCE,
            HttpField::KeepAlive => &KEEPALIVE,
            HttpField::LastModified => &LAST_MODIFIED,
            HttpField::Link => &LINK,
            HttpField::Location => &LOCATION,
            HttpField::MaxForwards => &MAX_FORWARDS,
            HttpField::Origin => &ORIGIN,
            HttpField::ProxyAuthenticate => &PROXY_AUTHENTICATE,
            HttpField::ProxyAuthorization => &PROXY_AUTHORIZATION,
            HttpField::Range => &RANGE,
            HttpField::Referer => &REFERER,
            HttpField::ReferrerPolicy => &REFERRER_POLICY,
            HttpField::RetryAfter => &RETRY_AFTER,
            HttpField::Server => &SERVER,
            HttpField::ServerTiming => &SERVER_TIMING,
            HttpField::SetCookie => &SET_COOKIE,
            HttpField::SourceMap => &SOURCE_MAP,
            HttpField::TE => &TE,
            HttpField::Trailer => &TRAILER,
            HttpField::TransferEncoding => &TRANSFER_ENCODING,
            HttpField::Upgrade => &UPGRADE,
            HttpField::UserAgent => &USER_AGENT,
            HttpField::Vary => &VARY,
            HttpField::Via => &VIA,
            HttpField::WWWAuthenticate => &WWW_AUTHENTICATE,
            HttpField::XRealIp => &X_REAL_IP,
            HttpField::XForwardedFor => &X_FORWARDED_FOR,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        let (s, _) = self.get();
        s.as_bytes()
    }

    pub fn as_str(&self) -> &str {
        let (s, _) = self.get();
        s.as_ref()
    }
}

impl Into<Cachestr> for HttpField {
    fn into(self) -> Cachestr {
        let (s, _) = self.get();
        Clone::clone(s)
    }
}

impl Into<u16> for HttpField {
    fn into(self) -> u16 {
        let (_, m) = self.get();
        *m
    }
}

impl FromStr for HttpField {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use strum::IntoEnumIterator;
        for next in Self::iter() {
            if next.as_str().eq_ignore_ascii_case(s) {
                return Ok(next);
            }
        }
        bail!("invalid http field '{}'", s)
    }
}

#[cfg(test)]
mod misc_tests {
    use strum::IntoEnumIterator;

    use super::*;

    fn init() {
        pretty_env_logger::try_init_timed().ok();
    }

    #[test]
    fn test_well_known_headers() {
        init();

        let mut s = vec![];
        for next in HttpField::iter() {
            s.push(next.as_str().to_lowercase().to_string())
        }

        info!("{}", s.len());

        let s = serde_json::to_string(&s);
        info!("{}", s.unwrap());
    }
}
