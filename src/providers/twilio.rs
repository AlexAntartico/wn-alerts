use async_trait::async_trait;
use url::Url;

use crate::core::incident::Incident;
use crate::core::provider::StatusProvider;
use crate::error::AppError;

const ALLOWED_DOMAINS: &[&str] = &["status.twilio.com"];

pub struct TwilioProvider {
    api_url: String,
    skip_validation: bool,
}

impl TwilioProvider {
    pub fn new(api_url: String) -> Self {
        Self {
            api_url,
            skip_validation: false,
        }
    }

    pub fn new_unvalidated(api_url: String) -> Self {
        Self {
            api_url,
            skip_validation: true,
        }
    }

    fn validate_url(&self) -> Result<(), AppError> {
        let parsed = Url::parse(&self.api_url).map_err(|e| AppError::InvalidConfig {
            key: "PROVIDER_TWILIO_API_URL",
            value: format!("{} (parse error: {})", self.api_url, e),
        })?;

        let scheme = parsed.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(AppError::InvalidConfig {
                key: "PROVIDER_TWILIO_API_URL",
                value: format!("{} (invalid scheme: {})", self.api_url, scheme),
            });
        }

        let host = parsed.host_str().ok_or_else(|| AppError::InvalidConfig {
            key: "PROVIDER_TWILIO_API_URL",
            value: format!("{} (no host)", self.api_url),
        })?;

        let is_allowed = ALLOWED_DOMAINS.iter().any(|&allowed| {
            host == allowed || host.ends_with(&format!(".{}", allowed))
        });

        if !is_allowed {
            return Err(AppError::InvalidConfig {
                key: "PROVIDER_TWILIO_API_URL",
                value: format!(
                    "{} (host '{}' not in allowed domains: {:?})",
                    self.api_url, host, ALLOWED_DOMAINS
                ),
            });
        }

        let is_private_ip =
            host.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| match ip {
                    std::net::IpAddr::V4(ipv4) => {
                        ipv4.is_loopback()
                            || ipv4.is_private()
                            || ipv4.is_link_local()
                            || ipv4.is_unspecified()
                            || ipv4.is_broadcast()
                    }
                    std::net::IpAddr::V6(ipv6) => ipv6.is_loopback() || ipv6.is_unspecified(),
                });

        if is_private_ip {
            return Err(AppError::InvalidConfig {
                key: "PROVIDER_TWILIO_API_URL",
                value: format!("{} (private/internal IP not allowed)", self.api_url),
            });
        }

        Ok(())
    }
}

#[async_trait]
impl StatusProvider for TwilioProvider {
    fn name(&self) -> &'static str {
        "twilio"
    }

    async fn check(&self, client: &reqwest::Client) -> Result<Vec<Incident>, AppError> {
        if !self.skip_validation {
            self.validate_url()?;
        }

        let response = client.get(&self.api_url).send().await?.error_for_status()?;
        let bytes = response.bytes().await?;
        json_parser::parse_incidents_from_bytes(&bytes)
    }
}

mod json_parser {
    use serde::Deserialize;

    use crate::core::incident::Incident;
    use crate::error::AppError;

    #[derive(Debug, Deserialize)]
    struct ApiResponse {
        #[serde(default)]
        incidents: Vec<ApiIncident>,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct ApiIncident {
        id: String,
        name: String,
        status: String,
        impact: String,
        shortlink: String,
        #[serde(default, rename = "incident_updates")]
        incident_updates: Vec<IncidentUpdate>,
    }

    #[derive(Debug, Deserialize)]
    struct IncidentUpdate {
        id: String,
        #[serde(default)]
        body: String,
        status: String,
        #[serde(alias = "created_at")]
        created_at: String,
    }

    pub fn parse_incidents_from_bytes(bytes: &[u8]) -> Result<Vec<Incident>, AppError> {
        let response: ApiResponse = serde_json::from_slice(bytes)?;

        let mut incidents = Vec::new();
        for api_incident in &response.incidents {
            for update in &api_incident.incident_updates {
                incidents.push(Incident {
                    id: format!("twilio:{}:{}", api_incident.id, update.id),
                    provider: "twilio".to_string(),
                    title: api_incident.name.clone(),
                    description: format!(
                        "[{}] [{}] {}",
                        api_incident.impact, update.status, update.body
                    ),
                    link: api_incident.shortlink.clone(),
                    occurred_at: update.created_at.clone(),
                });
            }
        }

        Ok(incidents)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_empty_response() {
            let json = br#"{"incidents":[]}"#;
            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert!(incidents.is_empty());
        }

        #[test]
        fn parse_response_with_no_incidents_field() {
            let json = br#"{"page":{}}"#;
            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert!(incidents.is_empty());
        }

        #[test]
        fn parse_incident_with_updates() {
            let json = br#"{
  "incidents": [
    {
      "id": "cp306tmzcl0y",
      "name": "Unplanned Database Outage",
      "status": "identified",
      "impact": "major",
      "shortlink": "http://stspg.co/Q0E",
      "incident_updates": [
        {
          "id": "jdy3tw5mt5r5",
          "body": "Our master database is experiencing issues.",
          "status": "investigating",
          "created_at": "2014-05-14T14:22:39.441-06:00"
        },
        {
          "id": "abc123",
          "body": "We have identified the root cause.",
          "status": "identified",
          "created_at": "2014-05-14T14:35:21.711-06:00"
        }
      ]
    }
  ]
}"#;

            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert_eq!(incidents.len(), 2);

            assert_eq!(incidents[0].id, "twilio:cp306tmzcl0y:jdy3tw5mt5r5");
            assert_eq!(incidents[0].provider, "twilio");
            assert_eq!(incidents[0].title, "Unplanned Database Outage");
            assert!(incidents[0]
                .description
                .contains("[major] [investigating]"));
            assert_eq!(incidents[0].link, "http://stspg.co/Q0E");
            assert_eq!(
                incidents[0].occurred_at,
                "2014-05-14T14:22:39.441-06:00"
            );

            assert_eq!(incidents[1].id, "twilio:cp306tmzcl0y:abc123");
            assert!(incidents[1]
                .description
                .contains("[major] [identified]"));
        }

        #[test]
        fn parse_incident_without_updates() {
            let json = br#"{
  "incidents": [
    {
      "id": "cp001",
      "name": "SMS Delivery Delays",
      "status": "resolved",
      "impact": "minor",
      "shortlink": "http://stspg.co/ABCD",
      "incident_updates": []
    }
  ]
}"#;

            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert!(incidents.is_empty());
        }

        #[test]
        fn parse_incident_with_missing_impact_field() {
            let json = br#"{
  "incidents": [
    {
      "id": "cp002",
      "name": "Voice Quality Issues",
      "status": "investigating",
      "impact": "",
      "shortlink": "http://stspg.co/EFGH",
      "incident_updates": [
        {
          "id": "upd001",
          "body": "Investigating reports.",
          "status": "investigating",
          "created_at": "2024-01-01T00:00:00Z"
        }
      ]
    }
  ]
}"#;

            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert_eq!(incidents.len(), 1);
            assert!(incidents[0].description.contains("[] [investigating]"));
        }

        #[test]
        fn parse_multiple_incidents() {
            let json = br#"{
  "incidents": [
    {
      "id": "cp010",
      "name": "API Errors",
      "status": "investigating",
      "impact": "critical",
      "shortlink": "http://stspg.co/API",
      "incident_updates": [
        {
          "id": "upd010",
          "body": "Investigating API errors in US1.",
          "status": "investigating",
          "created_at": "2024-06-01T10:00:00Z"
        }
      ]
    },
    {
      "id": "cp020",
      "name": "Console Login Issues",
      "status": "monitoring",
      "impact": "major",
      "shortlink": "http://stspg.co/CON",
      "incident_updates": [
        {
          "id": "upd020a",
          "body": "Login page returning 503.",
          "status": "investigating",
          "created_at": "2024-06-01T11:00:00Z"
        },
        {
          "id": "upd020b",
          "body": "Mitigation deployed, monitoring.",
          "status": "monitoring",
          "created_at": "2024-06-01T11:30:00Z"
        }
      ]
    }
  ]
}"#;

            let incidents = parse_incidents_from_bytes(json).unwrap();
            assert_eq!(incidents.len(), 3);
            assert_eq!(incidents[0].id, "twilio:cp010:upd010");
            assert_eq!(incidents[1].id, "twilio:cp020:upd020a");
            assert_eq!(incidents[2].id, "twilio:cp020:upd020b");
        }

        #[test]
        fn parse_garbage_bytes() {
            assert!(parse_incidents_from_bytes(b"not json").is_err());
        }

        #[test]
        fn parse_invalid_json_structure() {
            assert!(parse_incidents_from_bytes(br#"{"incidents":"not_an_array"}"#).is_err());
        }
    }
}

#[cfg(test)]
mod url_validation_tests {
    use super::*;

    #[test]
    fn validates_allowed_domain() {
        let provider = TwilioProvider::new(
            "https://status.twilio.com/api/v2/incidents.json".into(),
        );
        assert!(provider.validate_url().is_ok());
    }

    #[test]
    fn validates_subdomain() {
        let provider =
            TwilioProvider::new("https://sub.status.twilio.com/api/v2/incidents.json".into());
        assert!(provider.validate_url().is_ok());
    }

    #[test]
    fn rejects_disallowed_domains() {
        let provider = TwilioProvider::new("https://evil.example.com/api".into());
        let result = provider.validate_url();
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::InvalidConfig { key, value } => {
                assert_eq!(key, "PROVIDER_TWILIO_API_URL");
                assert!(value.contains("not in allowed domains"));
            }
            other => panic!("expected InvalidConfig, got {:?}", other),
        }
    }

    #[test]
    fn rejects_private_ips() {
        let provider = TwilioProvider::new("http://127.0.0.1/api".into());
        assert!(provider.validate_url().is_err());

        let provider = TwilioProvider::new("http://10.0.0.1/api".into());
        assert!(provider.validate_url().is_err());
    }

    #[test]
    fn rejects_invalid_schemes() {
        let provider = TwilioProvider::new("file:///etc/passwd".into());
        assert!(provider.validate_url().is_err());
    }

    #[test]
    fn unvalidated_provider_skips_validation() {
        let provider = TwilioProvider::new_unvalidated("https://evil.com/api".into());
        assert!(provider.skip_validation);
    }
}
