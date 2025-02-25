use custom_logger::*;
use http_body_util::{BodyExt, Full};
use hyper::body::*;
use hyper::{Method, Request, Response};
use ollama_rs::Ollama;
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};
use std::fs;
use std::str;

use crate::api::schema::*;
use crate::qdrant::client::*;

// pub type Result<T> = core::result::Result<T, Error>;

// pub type Error = Box<dyn std::error::Error>;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QueryDetails {
    #[serde(rename = "category")]
    pub category: String,

    #[serde(rename = "query")]
    pub query: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResponseDetails {
    #[serde(rename = "status")]
    pub status: String,

    #[serde(rename = "query")]
    pub query: Option<String>,

    #[serde(rename = "data")]
    pub data: String,

    #[serde(rename = "score")]
    pub score: String,
}

#[derive(Clone, Copy, Debug)]
pub struct ImplPayloadInterface {}

pub trait PayloadInterface {
    async fn payload(
        &self,
        log: &Logging,
        config: ApplicationConfig,
        data: String,
    ) -> Result<ResponseDetails, Box<dyn std::error::Error>>;
}

impl PayloadInterface for ImplPayloadInterface {
    async fn payload(
        &self,
        log: &Logging,
        config: ApplicationConfig,
        query: String,
    ) -> Result<ResponseDetails, Box<dyn std::error::Error>> {
        let result: ResponseDetails;
        // use config to create both
        // ollama client and qdrant client
        // setup qdrant client
        let client = Qdrant::from_url(&format!(
            "{}:{}",
            config.clone().spec.qdrant_url,
            config.clone().spec.qdrant_port
        ))
        .build();

        if client.is_err() {
            let res_err = ResponseDetails {
                status: "KO".to_string(),
                query: None,
                score: 0.0.to_string(),
                data: format!("qdrant {:#?}", client.err().unwrap()),
            };
            return Ok(res_err);
        }

        let qclient = VectorDB::new(client.unwrap());
        let ollama = Ollama::new(config.spec.ollama_url, config.spec.ollama_port as u16);
        log.debug(&format!("ollama connection {:#?}", ollama));

        let res = ollama
            .generate_embeddings(config.spec.model, query.clone(), None)
            .await;
        if res.is_err() {
            let res_err = ResponseDetails {
                status: "KO".to_string(),
                query: None,
                score: 0.0.to_string(),
                data: format!("ollama {:#?}", res.err().unwrap()),
            };
            return Ok(res_err);
        }

        let vecdb_res = qclient.search(config.spec.category, res.unwrap()).await?;
        if !vecdb_res.payload.is_empty() {
            log.info(&format!("score {:#?}", vecdb_res.score));
            if vecdb_res.score > config.spec.score_threshold {
                let v = vecdb_res.payload["id"].as_str().unwrap().clone();
                let markdown_data = fs::read_to_string(v)?;
                result = ResponseDetails {
                    status: "OK".to_string(),
                    query: Some(query.clone()),
                    score: vecdb_res.score.clone().to_string(),
                    data: markdown_data,
                };
            } else {
                result = ResponseDetails {
                    status: "KO".to_string(),
                    query: Some(query.clone()),
                    score: 0.0.to_string(),
                    data: "I could not find any related info, please refine your prompt"
                        .to_string(),
                };
            }
        } else {
            result = ResponseDetails {
                status: "KO".to_string(),
                query: Some(query.clone()),
                score: 0.0.to_string(),
                data: "I could not find any related info, please refine your prompt".to_string(),
            };
        }
        Ok(result)
    }
}

/// handler - reads json as input
pub async fn process_payload<T: PayloadInterface>(
    req: Request<hyper::body::Incoming>,
    log: &Logging,
    config: ApplicationConfig,
    q: T,
) -> Result<Response<Full<Bytes>>, Box<dyn std::error::Error + Send + Sync>> {
    match (req.method(), req.uri().path()) {
        (&Method::POST, "/query") => {
            let max = req.body().size_hint().upper().unwrap_or(u64::MAX);
            if max > 1024 * 64 {
                let resp_details = ResponseDetails {
                    status: "KO".to_string(),
                    score: 0.0.to_string(),
                    query: None,
                    data: "body too big".to_string(),
                };
                let resp_json = serde_json::to_string(&resp_details).unwrap();
                let mut resp = Response::new(Full::new(Bytes::from(resp_json)));
                *resp.status_mut() = hyper::StatusCode::PAYLOAD_TOO_LARGE;
                return Ok(resp);
            }
            let req_body = req.collect().await?.to_bytes();
            let payload = String::from_utf8(req_body.to_vec()).unwrap();
            log.info(&format!("payload {:#?}", payload));
            let query_json: QueryDetails = serde_json::from_str(&payload).unwrap();
            let res = q.payload(log, config, query_json.query.clone()).await;
            let resp_json = serde_json::to_string(&res.as_ref().unwrap()).unwrap();
            let mut final_res = Response::new(Full::new(Bytes::from(resp_json)));
            if res.unwrap().status == "KO" {
                *final_res.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
            } else {
                *final_res.status_mut() = hyper::StatusCode::OK;
            }
            return Ok(final_res);
        }
        // health endpoint
        (&Method::GET, "/isalive") => {
            let resp_details = ResponseDetails {
                status: "OK".to_string(),
                score: 0.0.to_string(),
                query: None,
                data: "service is up".to_string(),
            };
            let resp_json = serde_json::to_string(&resp_details).unwrap();
            let mut final_resp = Response::new(Full::new(Bytes::from(resp_json)));
            *final_resp.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
            return Ok(final_resp);
        }
        // all other routes
        _ => {
            let resp_details = ResponseDetails {
                status: "KO".to_string(),
                score: 0.0.to_string(),
                query: None,
                data: "ensure you post to the /query endpoint with valid json".to_string(),
            };
            let resp_json = serde_json::to_string(&resp_details).unwrap();
            let mut final_resp = Response::new(Full::new(Bytes::from(resp_json)));
            *final_resp.status_mut() = hyper::StatusCode::NOT_FOUND;
            return Ok(final_resp);
        }
    }
}
