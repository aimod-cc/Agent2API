//! GIF87a/89a 的有界封装检查，不解码像素、不抽帧、不改写原字节。
//! 依据 CompuServe GIF89a §15–27：
//! https://giflib.sourceforge.net/gifstandard/GIF89a.html
//! 与 DeepSeek 插件原字节校验同口径；不将封装完整冒充像素可解码或上游成功。

use agent2api_provider_protocol as proto;

const MAX_STEPS: usize = 100_000;

#[derive(Clone, Copy)]
enum Next {
    Block,
    Lzw,
    Data(bool),
}

#[derive(Clone, Copy)]
enum Step {
    Header,
    Block,
    Descriptor,
    Lzw,
    Extension,
    Graphic,
    GraphicEnd,
    Application,
    PlainText,
    Data(bool),
    Skip(usize, Next),
    Done,
}

pub(super) struct Validator {
    step: Step,
    pending: [u8; 13],
    used: usize,
    steps: usize,
    width: u32,
    height: u32,
    global_colors: usize,
    has_image: bool,
    has_image_data: bool,
    pending_control: bool,
}

fn invalid() -> proto::WireError {
    super::invalid("DeepSeek GIF 图片结构无效或被截断，未登记资源")
}

fn word(bytes: &[u8], index: usize) -> u32 {
    u16::from_le_bytes([bytes[index], bytes[index + 1]]) as u32
}

fn colors(flags: u8) -> usize {
    if flags & 0x80 == 0 {
        0
    } else {
        1usize << ((flags & 7) + 1)
    }
}

impl Validator {
    pub(super) fn new() -> Self {
        Self {
            step: Step::Header,
            pending: [0; 13],
            used: 0,
            steps: 0,
            width: 0,
            height: 0,
            global_colors: 0,
            has_image: false,
            has_image_data: false,
            pending_control: false,
        }
    }

    pub(super) fn push(&mut self, mut bytes: &[u8]) -> Result<(), proto::WireError> {
        while !bytes.is_empty() {
            if let Step::Skip(left, next) = self.step {
                let count = left.min(bytes.len());
                bytes = &bytes[count..];
                self.step = if count < left {
                    Step::Skip(left - count, next)
                } else {
                    match next {
                        Next::Block => Step::Block,
                        Next::Lzw => Step::Lzw,
                        Next::Data(image) => Step::Data(image),
                    }
                };
                continue;
            }
            let required = match self.step {
                Step::Header | Step::PlainText => 13,
                Step::Descriptor => 9,
                Step::Graphic => 5,
                Step::Application => 12,
                Step::Done => {
                    return Err(super::invalid(
                        "DeepSeek GIF 结束标记后还有未知内容，未登记资源",
                    ))
                }
                _ => 1,
            };
            let count = (required - self.used).min(bytes.len());
            self.pending[self.used..self.used + count].copy_from_slice(&bytes[..count]);
            self.used += count;
            bytes = &bytes[count..];
            if self.used == required {
                self.steps += 1;
                if self.steps > MAX_STEPS {
                    return Err(super::limit(
                        "DeepSeek GIF 块数量超过本地安全处理上限，未登记资源",
                    ));
                }
                self.advance()?;
                self.used = 0;
            }
        }
        Ok(())
    }

    fn advance(&mut self) -> Result<(), proto::WireError> {
        let bytes = self.pending;
        self.step = match self.step {
            Step::Header => {
                if &bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a" {
                    return Err(invalid());
                }
                self.width = word(&bytes, 6);
                self.height = word(&bytes, 8);
                if self.width == 0 || self.height == 0 {
                    return Err(invalid());
                }
                self.global_colors = colors(bytes[10]);
                if self.global_colors > 0 {
                    Step::Skip(3 * self.global_colors, Next::Block)
                } else {
                    Step::Block
                }
            }
            Step::Block => match bytes[0] {
                0x2c => Step::Descriptor,
                0x21 => Step::Extension,
                0x3b if self.has_image && !self.pending_control => Step::Done,
                _ => return Err(invalid()),
            },
            Step::Descriptor => {
                let width = word(&bytes, 4);
                let height = word(&bytes, 6);
                let local_colors = colors(bytes[8]);
                if width == 0
                    || height == 0
                    || word(&bytes, 0) + width > self.width
                    || word(&bytes, 2) + height > self.height
                    || bytes[8] & 0x18 != 0
                    || local_colors == 0 && self.global_colors == 0
                {
                    return Err(invalid());
                }
                self.has_image_data = false;
                self.pending_control = false;
                if local_colors > 0 {
                    Step::Skip(3 * local_colors, Next::Lzw)
                } else {
                    Step::Lzw
                }
            }
            Step::Lzw => {
                if !(2..=8).contains(&bytes[0]) {
                    return Err(invalid());
                }
                Step::Data(true)
            }
            Step::Extension => match bytes[0] {
                0xf9 => Step::Graphic,
                0xfe => Step::Data(false),
                0xff => Step::Application,
                0x01 => Step::PlainText,
                _ => {
                    return Err(super::invalid(
                        "DeepSeek GIF 含有尚未识别的扩展块，未登记资源",
                    ))
                }
            },
            Step::Graphic => {
                if bytes[0] != 4
                    || bytes[1] & 0xe0 != 0
                    || (bytes[1] >> 2) & 7 > 3
                    || self.pending_control
                {
                    return Err(invalid());
                }
                self.pending_control = true;
                Step::GraphicEnd
            }
            Step::GraphicEnd => {
                if bytes[0] != 0 {
                    return Err(invalid());
                }
                Step::Block
            }
            Step::Application => {
                if bytes[0] != 11 {
                    return Err(invalid());
                }
                Step::Data(false)
            }
            Step::PlainText => {
                if bytes[0] != 12
                    || self.global_colors == 0
                    || word(&bytes, 5) == 0
                    || word(&bytes, 7) == 0
                    || bytes[9] == 0
                    || bytes[10] == 0
                    || word(&bytes, 1) + word(&bytes, 5) > self.width
                    || word(&bytes, 3) + word(&bytes, 7) > self.height
                    || bytes[11] as usize >= self.global_colors
                    || bytes[12] as usize >= self.global_colors
                {
                    return Err(invalid());
                }
                self.pending_control = false;
                Step::Data(false)
            }
            Step::Data(image) => {
                if bytes[0] == 0 {
                    if image {
                        if !self.has_image_data {
                            return Err(invalid());
                        }
                        self.has_image = true;
                    }
                    Step::Block
                } else {
                    if image {
                        self.has_image_data = true;
                    }
                    Step::Skip(bytes[0] as usize, Next::Data(image))
                }
            }
            Step::Skip(..) | Step::Done => return Err(invalid()),
        };
        Ok(())
    }

    pub(super) fn finish(self) -> Result<(), proto::WireError> {
        if self.used == 0 && matches!(self.step, Step::Done) && self.has_image {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
