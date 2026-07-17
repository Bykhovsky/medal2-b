use nom::{
    bytes::complete::take,
    error::{Error, ErrorKind},
    number::complete::le_u8,
    Err, IResult,
};

use super::chunk::Chunk;

pub const MIN_SUPPORTED_VERSION: u8 = 4;
pub const MAX_SUPPORTED_VERSION: u8 = 9;

#[derive(Debug)]
pub enum Bytecode {
    Error(String),
    Chunk(Chunk),
}

impl Bytecode {
    pub fn parse(input: &[u8], encode_key: u8) -> IResult<&[u8], Bytecode> {
        let (input, status_code) = le_u8(input)?;
        match status_code {
            0 => {
                let (input, error_msg) = take(input.len())(input)?;
                Ok((
                    input,
                    Bytecode::Error(String::from_utf8_lossy(error_msg).to_string()),
                ))
            }
            MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION => {
                let (input, chunk) = Chunk::parse(input, encode_key, status_code)?;
                Ok((input, Bytecode::Chunk(chunk)))
            }
            _ => Err(Err::Failure(Error::new(input, ErrorKind::Verify))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_chunk(version: u8) -> Vec<u8> {
        // version, type version, string count, proto count, main proto id
        vec![version, 1, 0, 0, 0]
    }

    #[test]
    fn accepts_supported_versions_through_nine() {
        for version in MIN_SUPPORTED_VERSION..=MAX_SUPPORTED_VERSION {
            let input = empty_chunk(version);
            let (remaining, parsed) = Bytecode::parse(&input, 1).unwrap();

            assert!(remaining.is_empty());
            assert!(matches!(parsed, Bytecode::Chunk(_)));
        }
    }

    #[test]
    fn rejects_unsupported_versions_without_panicking() {
        let result = std::panic::catch_unwind(|| Bytecode::parse(&[10], 1));

        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}
