use aead::consts::U16;
use aead::generic_array::GenericArray;
use aead::{Key, KeyInit, KeySizeUser};
use anyhow::{Result, bail};
use cipher::{
    BlockCipher, BlockEncrypt, BlockSizeUser, InnerIvInit, StreamCipher, StreamCipherSeek,
};
use ctr::Ctr32BE;
use ghash::GHash;
use ghash::universal_hash::UniversalHash;

const TAG_SIZE: usize = 16;

#[derive(Clone)]
struct GcmGhash {
    ghash: GHash,
    ghash_pad: [u8; TAG_SIZE],
    msg_buf: [u8; TAG_SIZE],
    msg_buf_offset: usize,
    ad_buf: Vec<u8>,
    ad_len: usize,
    msg_len: usize,
}

impl GcmGhash {
    fn new(h: &[u8], ghash_pad: [u8; TAG_SIZE]) -> Result<Self, ()> {
        let ghash = GHash::new(h.into());

        Ok(Self {
            ghash,
            ghash_pad,
            msg_buf: [0u8; TAG_SIZE],
            msg_buf_offset: 0,
            ad_buf: Vec::new(),
            ad_len: 0,
            msg_len: 0,
        })
    }

    fn update_aad(&mut self, aad: &[u8]) {
        self.ad_len += aad.len();
        self.ad_buf.extend_from_slice(aad);
    }

    fn set_aad(&mut self) {
        self.ghash.update_padded(self.ad_buf.as_slice())
    }

    fn update(&mut self, msg: &[u8]) {
        if self.msg_buf_offset > 0 {
            let taking = std::cmp::min(msg.len(), TAG_SIZE - self.msg_buf_offset);
            self.msg_buf[self.msg_buf_offset..self.msg_buf_offset + taking]
                .copy_from_slice(&msg[..taking]);
            self.msg_buf_offset += taking;
            assert!(self.msg_buf_offset <= TAG_SIZE);

            self.msg_len += taking;

            if self.msg_buf_offset == TAG_SIZE {
                self.ghash
                    .update(std::slice::from_ref(ghash::Block::from_slice(
                        &self.msg_buf,
                    )));
                self.msg_buf_offset = 0;
                return self.update(&msg[taking..]);
            } else {
                return;
            }
        }

        self.msg_len += msg.len();

        assert_eq!(self.msg_buf_offset, 0);
        let full_blocks = msg.len() / 16;
        let leftover = msg.len() - 16 * full_blocks;
        assert!(leftover < TAG_SIZE);
        if full_blocks > 0 {
            // Safety: Transmute [u8] to [[u8; 16]], like slice::as_chunks.
            // Then transmute [[u8; 16]] to [GenericArray<U16>], per repr(transparent).
            let blocks = unsafe {
                std::slice::from_raw_parts(msg[..16 * full_blocks].as_ptr().cast(), full_blocks)
            };
            assert_eq!(
                std::mem::size_of_val(blocks) + leftover,
                std::mem::size_of_val(msg)
            );
            self.ghash.update(blocks);
        }

        self.msg_buf[0..leftover].copy_from_slice(&msg[full_blocks * 16..]);
        self.msg_buf_offset = leftover;
        assert!(self.msg_buf_offset < TAG_SIZE);
    }

    fn finalize(mut self) -> GenericArray<u8, U16> {
        if self.msg_buf_offset > 0 {
            self.ghash
                .update_padded(&self.msg_buf[..self.msg_buf_offset]);
        }

        let mut final_block = [0u8; 16];
        final_block[..8].copy_from_slice(&(8 * self.ad_len as u64).to_be_bytes());
        final_block[8..].copy_from_slice(&(8 * self.msg_len as u64).to_be_bytes());

        self.ghash.update(&[final_block.into()]);
        let mut hash = self.ghash.finalize();

        for (i, b) in hash.iter_mut().enumerate() {
            *b ^= self.ghash_pad[i];
        }

        hash
    }
}

pub struct AesGcm<Aes>
where
    Aes: BlockCipher + BlockSizeUser<BlockSize = U16> + BlockEncrypt,
{
    /// Encryption cipher
    ctr: Ctr32BE<Aes>,

    /// GHASH authenticator
    ghash: GcmGhash,
}

impl<Aes> KeySizeUser for AesGcm<Aes>
where
    Aes: KeySizeUser + BlockCipher + BlockSizeUser<BlockSize = U16> + BlockEncrypt,
{
    type KeySize = Aes::KeySize;
}

impl<Aes> AesGcm<Aes>
where
    Aes: BlockCipher + BlockSizeUser<BlockSize = U16> + BlockEncrypt + KeyInit,
{
    pub fn new(key: &Key<Self>, nonce: &[u8]) -> Self {
        let cipher = Aes::new(key);
        let mut ghash_key = ghash::Key::default();
        cipher.encrypt_block(&mut ghash_key);

        let mut nonce_block = GenericArray::default();
        if nonce.len() == 12 {
            nonce_block[..nonce.len()].copy_from_slice(nonce);
        } else {
            let mut ghash = GHash::new(&ghash_key);
            ghash.update_padded(nonce);
            ghash.update_padded(&(8 * nonce.len() as u128).to_be_bytes());
            nonce_block.copy_from_slice(&ghash.finalize());

            for i in nonce_block.iter_mut().rev() {
                *i = i.wrapping_sub(1);
                if *i != 0xff {
                    break;
                }
            }
        }
        let mut ctr = ctr::Ctr32BE::from_core(ctr::CtrCore::inner_iv_init(cipher, &nonce_block));
        ctr.seek(Aes::block_size());

        let mut pad = [0u8; 16];
        ctr.apply_keystream(&mut pad);

        let ghash = GcmGhash::new(&ghash_key, pad).unwrap();
        Self { ctr, ghash }
    }
}

impl<Aes> AesGcm<Aes>
where
    Aes: BlockCipher + BlockSizeUser<BlockSize = U16> + BlockEncrypt + KeyInit,
{
    pub fn update_aad(&mut self, aad: &[u8]) {
        self.ghash.update_aad(aad);
    }

    pub fn encrypt(&mut self, block: &mut [u8]) -> Result<()> {
        if !self.ghash.ad_buf.is_empty() {
            self.ghash.set_aad();
            self.ghash.ad_buf = Vec::new();
        }

        if let Err(e) = self.ctr.try_apply_keystream(block) {
            bail!("Failed to encrypt data.\n{:?}", e)
        }
        self.ghash.update(block);

        Ok(())
    }

    pub fn decrypt(&mut self, block: &mut [u8]) -> Result<()> {
        if !self.ghash.ad_buf.is_empty() {
            self.ghash.set_aad();
            self.ghash.ad_buf = Vec::new();
        }

        self.ghash.update(block);
        if let Err(e) = self.ctr.try_apply_keystream(block) {
            bail!("Failed to decrypt data.\n{:?}", e)
        }

        Ok(())
    }

    pub fn finish(&self) -> GenericArray<u8, U16> {
        let ghash = self.ghash.clone();
        ghash.finalize()
    }
}

// Took some test cases from NIST SP 800-38D to perform
// some tests. I want to test if the cipher works and specifically
// if the cipher works when encrypting/decrypting in chunks. To test
// this, I just chunk up the given pt/ct in chunks of 5 bytes.
// https://csrc.nist.gov/Projects/cryptographic-algorithm-validation-program/CAVP-TESTING-BLOCK-CIPHER-MODES#GCMVS
#[cfg(test)]
mod tests {
    use aes::Aes256;
    use hex::decode;

    use super::*;

    #[test]
    fn test_decrypt() {
        // Count = 0
        // Key = 54e352ea1d84bfe64a1011096111fbe7668ad2203d902a01458c3bbd85bfce14
        // IV = df7c3bca00396d0c018495d9
        // CT = 426e0efc693b7be1f3018db7ddbb7e4d
        // AAD = 7e968d71b50c1f11fd001f3fef49d045
        // Tag = ee8257795be6a1164d7e1d2d6cac77a7
        // PT = 85fc3dfad9b5a8d3258e4fc44571bd3b

        let key =
            decode("54e352ea1d84bfe64a1011096111fbe7668ad2203d902a01458c3bbd85bfce14").unwrap();
        let iv = decode("df7c3bca00396d0c018495d9").unwrap();
        let expected_pt = decode("85fc3dfad9b5a8d3258e4fc44571bd3b").unwrap();
        let ct = decode("426e0efc693b7be1f3018db7ddbb7e4d").unwrap();
        let aad = decode("7e968d71b50c1f11fd001f3fef49d045").unwrap();
        let expected_tag = decode("ee8257795be6a1164d7e1d2d6cac77a7").unwrap();

        let mut cipher = AesGcm::<Aes256>::new(key.as_slice().into(), iv.as_slice());
        cipher.update_aad(aad.as_slice());

        let mut pt = ct.clone();
        cipher.decrypt(pt.as_mut_slice()).unwrap();

        let tag = cipher.finish();

        assert_eq!(expected_pt.as_slice(), pt.as_slice());
        assert_eq!(expected_tag.as_slice(), tag.as_slice());
    }

    #[test]
    fn test_decrypt_chunks() {
        // Count = 0
        // Key = aeb3830cb9ce31cae7b1d47511bb2d3dcc2131714ace202b21b98820e7079792
        // IV = e7e87c45ec0a94c8e92353f1
        // CT = b20542b61b8fa6f847198334cb82fdbcb2311be855a6b2b3662bdb06ff0796238bea092a8ea21b585d38ace950378f41224269
        // AAD = 07d9bb1fa3aea7ceeefbedae87dcd713
        // Tag = 3bdd1d0cc2bbcefffe0ed2121aecbd00
        // PT = b4d0ecc410c430b61c11a1a42802858a0e9ee12f9a912f2f6b0570c99177f6de4bd79830cf9efb30759055e1f70d21e3f74957

        let key =
            decode("aeb3830cb9ce31cae7b1d47511bb2d3dcc2131714ace202b21b98820e7079792").unwrap();
        let iv = decode("e7e87c45ec0a94c8e92353f1").unwrap();
        let expected_pt = decode("b4d0ecc410c430b61c11a1a42802858a0e9ee12f9a912f2f6b0570c99177f6de4bd79830cf9efb30759055e1f70d21e3f74957").unwrap();
        let ct = decode("b20542b61b8fa6f847198334cb82fdbcb2311be855a6b2b3662bdb06ff0796238bea092a8ea21b585d38ace950378f41224269").unwrap();
        let aad = decode("07d9bb1fa3aea7ceeefbedae87dcd713").unwrap();
        let expected_tag = decode("3bdd1d0cc2bbcefffe0ed2121aecbd00").unwrap();

        let mut cipher = AesGcm::<Aes256>::new(key.as_slice().into(), iv.as_slice());

        cipher.update_aad(&aad[..5]);
        cipher.update_aad(&aad[5..]);

        let mut full_pt = Vec::new();
        let mut tmp_pt = Vec::new();

        for (n, byte) in ct.iter().enumerate() {
            tmp_pt.push(*byte);

            if n % 5 == 0 {
                cipher.decrypt(tmp_pt.as_mut_slice()).unwrap();
                full_pt.extend_from_slice(tmp_pt.as_slice());
                tmp_pt.clear();
            }
        }

        if !tmp_pt.is_empty() {
            cipher.decrypt(tmp_pt.as_mut_slice()).unwrap();
            full_pt.extend_from_slice(tmp_pt.as_slice());
        }

        let tag = cipher.finish();

        assert_eq!(expected_pt.as_slice(), full_pt.as_slice());
        assert_eq!(expected_tag.as_slice(), tag.as_slice());
    }

    #[test]
    fn test_encrypt() {
        // Count = 0
        // Key = 83688deb4af8007f9b713b47cfa6c73e35ea7a3aa4ecdb414dded03bf7a0fd3a
        // IV = 0b459724904e010a46901cf3
        // PT = 33d893a2114ce06fc15d55e454cf90c3
        // AAD = 794a14ccd178c8ebfd1379dc704c5e208f9d8424
        // CT = cc66bee423e3fcd4c0865715e9586696
        // Tag = 0fb291bd3dba94a1dfd8b286cfb97ac5

        let key =
            decode("83688deb4af8007f9b713b47cfa6c73e35ea7a3aa4ecdb414dded03bf7a0fd3a").unwrap();
        let iv = decode("0b459724904e010a46901cf3").unwrap();
        let pt = decode("33d893a2114ce06fc15d55e454cf90c3").unwrap();
        let expected_ct = decode("cc66bee423e3fcd4c0865715e9586696").unwrap();
        let aad = decode("794a14ccd178c8ebfd1379dc704c5e208f9d8424").unwrap();
        let expected_tag = decode("0fb291bd3dba94a1dfd8b286cfb97ac5").unwrap();

        let mut cipher = AesGcm::<Aes256>::new(key.as_slice().into(), iv.as_slice());
        cipher.update_aad(aad.as_slice());

        let mut ct = pt.clone();
        cipher.encrypt(ct.as_mut_slice()).unwrap();

        let tag = cipher.finish();

        assert_eq!(expected_ct.as_slice(), ct.as_slice());
        assert_eq!(expected_tag.as_slice(), tag.as_slice());
    }

    #[test]
    fn test_encrypt_chunks() {
        // Count = 0
        // Key = 24501ad384e473963d476edcfe08205237acfd49b5b8f33857f8114e863fec7f
        // IV = 9ff18563b978ec281b3f2794
        // PT = 27f348f9cdc0c5bd5e66b1ccb63ad920ff2219d14e8d631b3872265cf117ee86757accb158bd9abb3868fdc0d0b074b5f01b2c
        // AAD = adb5ec720ccf9898500028bf34afccbcaca126ef
        // CT = eb7cb754c824e8d96f7c6d9b76c7d26fb874ffbf1d65c6f64a698d839b0b06145dae82057ad55994cf59ad7f67c0fa5e85fab8
        // Tag = bc95c532fecc594c36d1550286a7a3f0

        let key =
            decode("24501ad384e473963d476edcfe08205237acfd49b5b8f33857f8114e863fec7f").unwrap();
        let iv = decode("9ff18563b978ec281b3f2794").unwrap();
        let pt = decode("27f348f9cdc0c5bd5e66b1ccb63ad920ff2219d14e8d631b3872265cf117ee86757accb158bd9abb3868fdc0d0b074b5f01b2c").unwrap();
        let expected_ct = decode("eb7cb754c824e8d96f7c6d9b76c7d26fb874ffbf1d65c6f64a698d839b0b06145dae82057ad55994cf59ad7f67c0fa5e85fab8").unwrap();
        let aad = decode("adb5ec720ccf9898500028bf34afccbcaca126ef").unwrap();
        let expected_tag = decode("bc95c532fecc594c36d1550286a7a3f0").unwrap();

        let mut cipher = AesGcm::<Aes256>::new(key.as_slice().into(), iv.as_slice());
        cipher.update_aad(&aad[..5]);
        cipher.update_aad(&aad[5..]);

        let mut full_ct = Vec::new();
        let mut tmp_ct = Vec::new();

        for (n, byte) in pt.iter().enumerate() {
            tmp_ct.push(*byte);

            if n % 5 == 0 {
                cipher.encrypt(tmp_ct.as_mut_slice()).unwrap();
                full_ct.extend_from_slice(tmp_ct.as_slice());
                tmp_ct.clear();
            }
        }

        if !tmp_ct.is_empty() {
            cipher.decrypt(tmp_ct.as_mut_slice()).unwrap();
            full_ct.extend_from_slice(tmp_ct.as_slice());
        }

        let tag = cipher.finish();

        assert_eq!(expected_ct.as_slice(), full_ct.as_slice());
        assert_eq!(expected_tag.as_slice(), tag.as_slice());
    }
}
