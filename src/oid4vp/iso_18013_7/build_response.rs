use anyhow::{bail, Context, Result};
use base64::prelude::*;
use isomdl::{cbor, definitions::DeviceResponse};
use josekit::{
    jwe::{alg::ecdh_es::EcdhEsJweEncrypter, JweHeader},
    jwk::Jwk,
    jwt::{encode_with_encrypter, JwtPayload},
};
use openid4vp::{
    core::{
        authorization_request::AuthorizationRequestObject,
        credential_format::ClaimFormatDesignation::MsoMDoc,
        object::ParsingErrorContext,
        presentation_definition::PresentationDefinition,
        presentation_submission::{DescriptorMap, PresentationSubmission},
        response::{parameters::State, AuthorizationResponse, JwtAuthorizationResponse},
    },
    JsonPath,
};
use p256::NistP256;
use serde_json::{json, Value as Json};
use uuid::Uuid;

const SUPPORTED_ALG: &str = "ECDH-ES";
const SUPPORTED_ENC: &str = "A256GCM";

pub fn build_response(
    request: &AuthorizationRequestObject,
    presentation_definition: &PresentationDefinition,
    device_response: DeviceResponse,
    mdoc_generated_nonce: String,
) -> Result<AuthorizationResponse> {
    let descriptor_map = DescriptorMap {
        id: "int.icao.epl.1".to_string(),
        format: MsoMDoc,
        path: JsonPath::default(),
        path_nested: None,
    };
    let presentation_submission = PresentationSubmission::new(
        Uuid::new_v4(),
        presentation_definition.id().clone(),
        vec![descriptor_map],
    );

    let device_response = BASE64_URL_SAFE_NO_PAD.encode(
        cbor::to_vec(&device_response).context("failed to encode device response as CBOR")?,
    );

    let apu = &mdoc_generated_nonce;
    let apv = request.nonce().as_str();
    let vp_token = Json::String(device_response);

    let jwe = build_jwe(request, vp_token, &presentation_submission, apu, apv)?;

    let authorization_response =
        AuthorizationResponse::Jwt(JwtAuthorizationResponse { response: jwe });

    Ok(authorization_response)
}

fn build_jwe(
    request: &AuthorizationRequestObject,
    vp_token: Json,
    presentation_submission: &PresentationSubmission,
    apu: &str,
    apv: &str,
) -> Result<String> {
    let client_metadata = request
        .client_metadata()
        .context("failed to resolve client_metadata")?;

    let alg = client_metadata
        .authorization_encrypted_response_alg()
        .parsing_error()?
        .0;
    if alg != SUPPORTED_ALG {
        bail!("unsupported encryption alg: {alg}")
    }

    let enc = client_metadata
        .authorization_encrypted_response_enc()
        .parsing_error()?
        .0;
    if enc != SUPPORTED_ENC {
        bail!("unsupported encryption scheme: {enc}")
    }

    let jwk = client_metadata
        .jwks()
        .parsing_error()?
        .keys
        .into_iter()
        .filter_map(|jwk| {
            let jwk = serde_json::from_value::<Jwk>(Json::Object(jwk));
            match jwk {
                Ok(jwk) => Some(jwk),
                Err(e) => {
                    tracing::warn!("unable to parse a JWK in keyset: {e}");
                    None
                }
            }
        })
        .find(|jwk| {
            let Some(crv) = jwk.curve() else {
                tracing::warn!("jwk in keyset was missing 'crv'");
                return false;
            };
            if let Some(use_) = jwk.key_use() {
                crv == "P-256" && use_ == "enc"
            } else {
                tracing::warn!("jwk in keyset was missing 'use'");
                crv == "P-256"
            }
        })
        .context("no 'P-256' keys for use 'enc' found in JWK keyset")?;

    let mut jwe_header = JweHeader::new();

    jwe_header.set_token_type("JWT");
    jwe_header.set_content_encryption(SUPPORTED_ENC);
    jwe_header.set_algorithm(SUPPORTED_ALG);
    jwe_header.set_agreement_partyuinfo(apu);
    jwe_header.set_agreement_partyvinfo(apv);

    if let Some(kid) = jwk.key_id() {
        jwe_header.set_key_id(kid);
    }

    let mut jwe_payload = JwtPayload::new();
    jwe_payload.set_claim("vp_token", Some(vp_token))?;
    jwe_payload.set_claim(
        "presentation_submission",
        Some(json!(presentation_submission)),
    )?;

    if let Some(state) = request.get::<State>() {
        jwe_payload.set_claim(
            "state",
            Some(serde_json::Value::String(state.parsing_error()?.0)),
        )?;
    }

    tracing::debug!(
        "JWE payload:\n{}",
        serde_json::to_string_pretty(jwe_payload.as_ref()).unwrap()
    );

    let encrypter: EcdhEsJweEncrypter<NistP256> = josekit::jwe::ECDH_ES.encrypter_from_jwk(&jwk)?;

    let jwe = encode_with_encrypter(&jwe_payload, &jwe_header, &encrypter)?;
    tracing::debug!("JWE: {jwe}");

    Ok(jwe)
}
