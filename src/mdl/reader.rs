use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use isomdl::{
    definitions::{
        device_request,
        helpers::{non_empty_map, NonEmptyMap},
        x509::{
            self,
            trust_anchor::{PemTrustAnchor, TrustAnchorRegistry},
        },
    },
    presentation::{authentication::AuthenticationStatus as IsoMdlAuthenticationStatus, reader},
};
use uuid::{uuid, Uuid};

#[derive(thiserror::Error, uniffi::Error, Debug)]
pub enum MDLReaderSessionError {
    #[error("{value}")]
    Generic { value: String },
}

#[derive(uniffi::Object)]
pub struct MDLSessionManager(reader::SessionManager);

impl std::fmt::Debug for MDLSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Debug for SessionManager not implemented")
    }
}

// Added by Warren Gallagher at AffinitiQuest
#[derive(uniffi::Enum)]
pub enum MDLSessionMode {
    CentralClientMode,
    PeripheralServerMode
}

#[derive(uniffi::Record)]
pub struct MDLReaderSessionData {
    pub state: Arc<MDLSessionManager>,
    uuid: Uuid,
    pub request: Vec<u8>,
    ble_ident: Vec<u8>,
    pub mode: MDLSessionMode, // Added by Warren Gallagher at AffinitiQuest
}

#[uniffi::export]
pub fn establish_session(
    uri: String,
    requested_items: HashMap<String, HashMap<String, bool>>,
    trust_anchor_registry: Option<Vec<String>>,
) -> Result<MDLReaderSessionData, MDLReaderSessionError> {
    let namespaces: Result<BTreeMap<_, NonEmptyMap<_, _>>, non_empty_map::Error> = requested_items
        .into_iter()
        .map(|(doc_type, namespaces)| {
            let namespaces: BTreeMap<_, _> = namespaces.into_iter().collect();
            match namespaces.try_into() {
                Ok(n) => Ok((doc_type, n)),
                Err(e) => Err(e),
            }
        })
        .collect();
    let namespaces = namespaces.map_err(|e| MDLReaderSessionError::Generic {
        value: format!("Unable to build data elements: {e:?}"),
    })?;
    let namespaces: device_request::Namespaces =
        namespaces
            .try_into()
            .map_err(|e| MDLReaderSessionError::Generic {
                value: format!("Unable to build namespaces: {e:?}"),
            })?;

    let registry = TrustAnchorRegistry::from_pem_certificates(
        trust_anchor_registry
            .into_iter()
            .flat_map(|v| v.into_iter())
            .map(|certificate_pem| PemTrustAnchor {
                certificate_pem,
                purpose: x509::trust_anchor::TrustPurpose::Iaca,
            })
            .collect(),
    )
    .map_err(|e| MDLReaderSessionError::Generic {
        value: format!("unable to construct TrustAnchorRegistry: {e:?}"),
    })?;

    let (manager, request, ble_ident) =
        reader::SessionManager::establish_session(uri.to_string(), namespaces, registry).map_err(
            |e| MDLReaderSessionError::Generic {
                value: format!("unable to establish session: {e:?}"),
            },
        )?;
        
    let manager2 = manager.clone();

    let uuid = manager2.first_peripheral_server_uuid();
    println!("{:#?}", uuid);

    // let qr_code = uri.to_string();
    // let device_engagement_bytes = Tag24::<DeviceEngagement>::from_qr_code_uri(&qr_code)
    //     .context("failed to construct QR code")?;
        
    // manager.session_transcript
    //     .0
    //     .as_ref()
    //     .device_retrieval_methods
    //     .as_ref()
    //     .and_then(|ms| {
    //         ms.as_ref()
    //             .iter()
    //             .filter_map(|m| match m {
    //                 _ => Err(MDLReaderSessionError::Generic {
    //                     value: opt.to_string(),
    //                 });
    //                 // DeviceRetrievalMethod::BLE(opt) => {
    //                 //     opt.central_client_mode.as_ref().map(|cc| &cc.uuid)
    //                 // }
    //                 // _ => None,
    //             })
    //             .next()
    //     })
    
    // Based on the BLE options provided in the QR code from the mdl (holder/wallet), it prefers to be:
    //  * use BLE in Peripheral Server Mode OR
    //  * use BLE in Central Client Mode
    // if the mdl specifies both, then the Reader shall use Central Client Mode
    // let manager2 = manager.clone();
    // let uuid  = manager2.first_central_client_uuid();
    // if uuid.is_none() {
    //    let uuid = manager2.first_central_client_uuid();//.first_peripheral_server_uuid();
    //    if uuid.is_none() {
    //        return Err(MDLReaderSessionError::Generic {
    //            value: "the device did not transmit a central client uuid".to_string(),
    //        });
    //    }
    //    else {
    //        return Ok(MDLReaderSessionData {
    //            state: Arc::new(MDLSessionManager(manager)),
    //            request,
    //            ble_ident: ble_ident.to_vec(),
    //            uuid: *uuid.unwrap(),
    //            mode: MDLSessionMode::PeripheralServerMode, // mdl (wallet/holder) wants central client mode, so the Reader should use peripheral server mode
    //        });
    //    }
    // }

    Ok(MDLReaderSessionData {
        state: Arc::new(MDLSessionManager(manager)),
        request,
        ble_ident: ble_ident.to_vec(),
        uuid: *uuid.unwrap(),//uuid!("00006e50-0000-1000-8000-00805f9b34fb"),//Uuid::new_v4(),
        mode: MDLSessionMode::CentralClientMode, // mdl (wallet/holder) wants peripheral server mode, so the Reader should use central client mode
    })
}

#[derive(thiserror::Error, uniffi::Error, Debug, PartialEq)]
pub enum MDLReaderResponseError {
    #[error("Invalid decryption")]
    InvalidDecryption,
    #[error("Invalid parsing")]
    InvalidParsing,
    #[error("Invalid issuer authentication")]
    InvalidIssuerAuthentication,
    #[error("Invalid device authentication")]
    InvalidDeviceAuthentication,
    #[error("{value}")]
    Generic { value: String },
}

// Currently, a lot of information is lost in `isomdl`. For example, bytes are
// converted to strings, but we could also imagine detecting images and having
// a specific enum variant for them.
#[derive(uniffi::Enum, Debug)]
pub enum MDocItem {
    Text(String),
    Bool(bool),
    Integer(i64),
    ItemMap(HashMap<String, MDocItem>),
    Array(Vec<MDocItem>),
}

impl From<serde_json::Value> for MDocItem {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => unreachable!("No null allowed in namespaces"),
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Self::Integer(i)
                } else {
                    unreachable!("Only integers allowed in namespaces")
                }
            }
            serde_json::Value::String(s) => Self::Text(s),
            serde_json::Value::Array(a) => {
                Self::Array(a.iter().map(|o| Into::<Self>::into(o.clone())).collect())
            }
            serde_json::Value::Object(m) => Self::ItemMap(
                m.iter()
                    .map(|(k, v)| (k.clone(), Into::<Self>::into(v.clone())))
                    .collect(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum AuthenticationStatus {
    Valid,
    Invalid,
    Unchecked,
}

impl From<IsoMdlAuthenticationStatus> for AuthenticationStatus {
    fn from(internal: IsoMdlAuthenticationStatus) -> Self {
        match internal {
            IsoMdlAuthenticationStatus::Valid => AuthenticationStatus::Valid,
            IsoMdlAuthenticationStatus::Invalid => AuthenticationStatus::Invalid,
            IsoMdlAuthenticationStatus::Unchecked => AuthenticationStatus::Unchecked,
        }
    }
}
#[derive(uniffi::Record, Debug)]
pub struct MDLReaderResponseData {
    state: Arc<MDLSessionManager>,
    /// Contains the namespaces for the mDL directly, without top-level doc types
    verified_response: HashMap<String, HashMap<String, MDocItem>>,
    /// Outcome of issuer authentication.
    pub issuer_authentication: AuthenticationStatus,
    /// Outcome of device authentication.
    pub device_authentication: AuthenticationStatus,
    /// Errors that occurred during response processing.
    pub errors: Option<String>,
}

#[uniffi::export]
pub fn handle_response(
    state: Arc<MDLSessionManager>,
    response: Vec<u8>,
) -> Result<MDLReaderResponseData, MDLReaderResponseError> {
    let mut state = state.0.clone();
    let validated_response = state.handle_response(&response);
    println!("{:#?}", validated_response);
    let errors = if !validated_response.errors.is_empty() {
        Some(
            serde_json::to_string(&validated_response.errors).map_err(|e| {
                MDLReaderResponseError::Generic {
                    value: format!("Could not serialze errors: {e:?}"),
                }
            })?,
        )
    } else {
        None
    };
    println!("{:#?}", errors);
    let verified_response: Result<_, _> = validated_response
        .response
        .into_iter()
        .map(|(namespace, items)| {
            if let Some(items) = items.as_object() {
                let items = items
                    .iter()
                    .map(|(item, value)| (item.clone(), value.clone().into()))
                    .collect();
                Ok((namespace.to_string(), items))
            } else {
                Err(MDLReaderResponseError::Generic {
                    value: format!("Items not object, instead: {items:#?}"),
                })
            }
        })
        .collect();
    let verified_response = verified_response.map_err(|e| MDLReaderResponseError::Generic {
        value: format!("Unable to parse response: {e:?}"),
    })?;
    Ok(MDLReaderResponseData {
        state: Arc::new(MDLSessionManager(state)),
        verified_response,
        issuer_authentication: AuthenticationStatus::from(validated_response.issuer_authentication),
        device_authentication: AuthenticationStatus::from(validated_response.device_authentication),
        errors,
    })
}
