use std::io::{self, Read, Write};

use aead_stream::{DecryptorBE32, EncryptorBE32, Nonce, StreamBE32};
use aes_gcm::{Aes256Gcm, KeyInit};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

pub(super) const CHUNK_BYTES: usize = 1024 * 1024;
const TAG_BYTES: usize = 16;

type StreamNonce = Nonce<Aes256Gcm, StreamBE32<Aes256Gcm>>;

pub(super) struct EncryptWriter<W: Write> {
    writer: W,
    cipher: Option<EncryptorBE32<Aes256Gcm>>,
    buffer: Zeroizing<Vec<u8>>,
    header: Vec<u8>,
}

impl<W: Write> EncryptWriter<W> {
    pub fn new(writer: W, key: &[u8; 32], nonce: &[u8; 7], header: Vec<u8>) -> io::Result<Self> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| invalid("备份加密密钥无效"))?;
        let nonce = StreamNonce::from(*nonce);
        Ok(Self {
            writer,
            cipher: Some(EncryptorBE32::from_aead(cipher, &nonce)),
            buffer: Zeroizing::new(Vec::with_capacity(CHUNK_BYTES + TAG_BYTES)),
            header,
        })
    }

    fn emit_next(&mut self) -> io::Result<()> {
        self.cipher
            .as_mut()
            .ok_or_else(|| invalid("备份加密流已结束"))?
            .encrypt_next_in_place(&self.header, &mut *self.buffer)
            .map_err(|_| invalid("备份分段加密失败"))?;
        self.writer.write_all(&self.buffer)?;
        self.buffer.clear();
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<W> {
        self.cipher
            .take()
            .ok_or_else(|| invalid("备份加密流已结束"))?
            .encrypt_last_in_place(&self.header, &mut *self.buffer)
            .map_err(|_| invalid("备份末段加密失败"))?;
        self.writer.write_all(&self.buffer)?;
        self.writer.flush()?;
        self.buffer.zeroize();
        Ok(self.writer)
    }
}

impl<W: Write> Write for EncryptWriter<W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len();
        while !bytes.is_empty() {
            let available = CHUNK_BYTES - self.buffer.len();
            let take = available.min(bytes.len());
            self.buffer.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if self.buffer.len() == CHUNK_BYTES {
                self.emit_next()?;
            }
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

pub(super) struct DecryptReader<R: Read> {
    reader: R,
    cipher: Option<DecryptorBE32<Aes256Gcm>>,
    buffer: Vec<u8>,
    position: usize,
    header: Vec<u8>,
    finished: bool,
}

impl<R: Read> DecryptReader<R> {
    pub fn new(reader: R, key: &[u8; 32], nonce: &[u8; 7], header: Vec<u8>) -> io::Result<Self> {
        let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| invalid("备份解密密钥无效"))?;
        let nonce = StreamNonce::from(*nonce);
        Ok(Self {
            reader,
            cipher: Some(DecryptorBE32::from_aead(cipher, &nonce)),
            buffer: Vec::with_capacity(CHUNK_BYTES + TAG_BYTES),
            position: 0,
            header,
            finished: false,
        })
    }

    fn refill(&mut self) -> io::Result<()> {
        self.buffer.zeroize();
        self.buffer.resize(CHUNK_BYTES + TAG_BYTES, 0);
        let mut count = 0;
        while count < self.buffer.len() {
            match self.reader.read(&mut self.buffer[count..]) {
                Ok(0) => break,
                Ok(read) => count += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        self.buffer.truncate(count);
        if count < TAG_BYTES {
            return Err(invalid("备份文件不完整或已被截断"));
        }
        let result = if count == CHUNK_BYTES + TAG_BYTES {
            self.cipher
                .as_mut()
                .ok_or_else(|| invalid("备份解密流已结束"))?
                .decrypt_next_in_place(&self.header, &mut self.buffer)
        } else {
            self.finished = true;
            self.cipher
                .take()
                .ok_or_else(|| invalid("备份解密流已结束"))?
                .decrypt_last_in_place(&self.header, &mut self.buffer)
        };
        if result.is_err() {
            self.buffer.zeroize();
            return Err(invalid("备份密码不正确，或备份文件已损坏"));
        }
        self.position = 0;
        Ok(())
    }

    pub fn finish(mut self) -> io::Result<()> {
        let mut discard = [0u8; 8192];
        if self.read(&mut discard)? != 0 {
            discard.zeroize();
            return Err(invalid("备份包含多余数据"));
        }
        discard.zeroize();
        Ok(())
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.position == self.buffer.len() {
            if self.finished {
                return Ok(0);
            }
            self.refill()?;
        }
        let count = output.len().min(self.buffer.len() - self.position);
        output[..count].copy_from_slice(&self.buffer[self.position..self.position + count]);
        self.position += count;
        Ok(count)
    }
}

impl<R: Read> Drop for DecryptReader<R> {
    fn drop(&mut self) {
        self.buffer.zeroize();
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
