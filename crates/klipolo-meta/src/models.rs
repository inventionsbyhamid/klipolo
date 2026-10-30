//! Meta's model list, used only to check a key before it is saved: a free call that needs a valid key.

use crate::{MetaClient, MetaError};

impl MetaClient {
    /// Whether Meta accepts this client's key: `GET /models`, which costs nothing. Any 2xx answer is a yes and the
    /// list itself isn't read; a refused key is [`MetaError::Unauthorized`]. Meant for the person's own key (a relay
    /// client would ask the relay's mirror, which may not offer it).
    pub async fn check_key(&self) -> Result<(), MetaError> {
        let request = self
            .http
            .get(self.meta_url("models"))
            .timeout(self.short_timeout);
        self.send_meta(request).await.map(drop)
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn a_key_is_checked_with_the_free_model_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-good"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\": []}"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer sk-bad"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = |key: &str| {
            MetaClient::direct(SecretString::from(key.to_owned()))
                .with_meta_base(format!("{}/v1", server.uri()))
                .with_ledger(None)
        };
        assert_eq!(client("sk-good").check_key().await, Ok(()));
        assert_eq!(
            client("sk-bad").check_key().await,
            Err(MetaError::Unauthorized)
        );
    }
}
