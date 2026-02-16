use ::rsa::pkcs1::DecodeRsaPublicKey;
use ::rsa::{Oaep, RsaPublicKey, rand_core};
use aes::Aes256;
use aes::cipher::crypto_common::rand_core::{OsRng, RngCore};
use pyo3::exceptions::{PyImportError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyModuleMethods;
use sha1::{Digest, Sha1};
use spki::Document;

use crate::aes_stream::{self, AesGcm as Cipher};

#[pymodule]
pub fn _pystandalone(py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Enforce minimum Python version requirement.
    if py.version_info() != (3, 11) {
        return Err(PyImportError::new_err("module requires Python 3.11"));
    }

    if let Err(e) = m.add_function(wrap_pyfunction!(rand_bytes, m)?) {
        let error = format!("Failed to import rand_bytes function.\n{}", e);
        return Err(PyImportError::new_err(error));
    }

    if let Err(e) = m.add_class::<rsa>() {
        let error = format!("Failed to import rsa class.\n{}", e);
        return Err(PyImportError::new_err(error));
    }

    if let Err(e) = m.add_class::<aes_256_gcm>() {
        let error = format!("Failed to import aes_256_gcm class.\n{}", e);
        return Err(PyImportError::new_err(error));
    }

    Ok(())
}

#[pyfunction]
pub fn rand_bytes(size: usize) -> PyResult<Vec<u8>> {
    let mut bytes: Vec<u8> = vec![0; size];
    OsRng.fill_bytes(bytes.as_mut_slice());

    Ok(bytes)
}

#[allow(non_camel_case_types)]
#[pyclass]
pub struct rsa {
    pubkey: RsaPublicKey,
}

#[allow(non_camel_case_types)]
#[pyclass]
pub struct aes_256_gcm {
    cipher: Cipher<Aes256>,
}

#[allow(non_camel_case_types)]
#[pyclass]
pub struct cipher {}

#[pymethods]
impl rsa {
    #[new]
    fn new(key: &str) -> PyResult<Self> {
        let pub_key = match RsaPublicKey::from_pkcs1_pem(key) {
            Ok(pub_key) => pub_key,
            Err(e) => {
                return Err(PyRuntimeError::new_err(format!(
                    "Failed to initialize RSA public key. {}",
                    e
                )));
            }
        };

        Ok(rsa { pubkey: pub_key })
    }

    fn encrypt(self_: PyRef<'_, Self>, bytes: &[u8]) -> PyResult<Vec<u8>> {
        let mut rng = rand_core::OsRng;
        let padding = Oaep {
            digest: Box::new(Sha1::new()),
            mgf_digest: Box::new(Sha1::new()),
            label: None,
        };

        match self_.pubkey.encrypt(&mut rng, padding, bytes) {
            Ok(ciphertext) => Ok(ciphertext),
            Err(e) => Err(PyRuntimeError::new_err(format!(
                "RSA encryption failed. {}",
                e
            ))),
        }
    }

    fn der(self_: PyRef<'_, Self>) -> PyResult<Vec<u8>> {
        match to_public_key_der(self_.pubkey.clone()) {
            Ok(bytes) => Ok(bytes.into_vec()),
            Err(e) => Err(PyRuntimeError::new_err(format!(
                "RSA DER conversion failed. {}",
                e
            ))),
        }
    }
}

#[pymethods]
impl aes_256_gcm {
    #[new]
    fn init(key: &[u8], iv: &[u8]) -> PyResult<Self> {
        Ok(aes_256_gcm {
            cipher: aes_stream::AesGcm::<Aes256>::new(key.into(), iv),
        })
    }

    fn update(mut self_: PyRefMut<'_, Self>, bytes: &[u8]) -> PyResult<()> {
        self_.cipher.update_aad(bytes);

        Ok(())
    }

    fn encrypt(mut self_: PyRefMut<'_, Self>, bytes: &[u8]) -> PyResult<Vec<u8>> {
        let mut bytes = bytes.to_vec();
        if let Err(e) = self_.cipher.encrypt(bytes.as_mut_slice()) {
            Err(PyRuntimeError::new_err(format!(
                "Failed to encrypt data.\n{:?}",
                e
            )))
        } else {
            Ok(bytes)
        }
    }

    fn decrypt(mut self_: PyRefMut<'_, Self>, bytes: &[u8]) -> PyResult<Vec<u8>> {
        let mut bytes = bytes.to_vec();

        if let Err(e) = self_.cipher.decrypt(bytes.as_mut_slice()) {
            Err(PyRuntimeError::new_err(format!(
                "Failed to decrypt data.\n{:?}",
                e
            )))
        } else {
            Ok(bytes)
        }
    }

    fn digest(self_: PyRef<'_, Self>) -> PyResult<Vec<u8>> {
        let tag = &self_.cipher.finish();

        Ok(tag.to_vec())
    }

    fn verify(self_: PyRef<'_, Self>, tag: &[u8]) -> PyResult<()> {
        let ghash1 = &self_.cipher.finish();

        if ghash1.as_slice() != tag {
            return Err(PyValueError::new_err(
                "Tag check failed. A mismatch occured",
            ));
        };

        Ok(())
    }
}

fn to_public_key_der(pubkey: RsaPublicKey) -> spki::Result<Document> {
    let subject_public_key = ::rsa::pkcs1::EncodeRsaPublicKey::to_pkcs1_der(&pubkey)?;

    ::rsa::pkcs8::SubjectPublicKeyInfoRef {
        algorithm: ::rsa::pkcs1::ALGORITHM_ID,
        subject_public_key: ::rsa::pkcs8::der::asn1::BitStringRef::new(
            0,
            subject_public_key.as_ref(),
        )?,
    }
    .try_into()
}
