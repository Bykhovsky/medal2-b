use std::convert::TryFrom;

use crate::op_code::OpCode;

/*

registers in () are not used
f prefix means no registers are used but its parsed as said type

LOP_NOP, f abc
LOP_BREAK, f abc
LOP_LOADNIL, a (bc)
LOP_LOADB, abc
LOP_LOADN, ad
LOP_LOADK, ad
LOP_MOVE, ab (c)
LOP_GETGLOBAL, a (b) c aux
LOP_SETGLOBAL, a (b) c aux
LOP_GETUPVAL, ab (c)
LOP_SETUPVAL, ab (c)
LOP_CLOSEUPVALS, a (bc)
LOP_GETIMPORT, ad aux
LOP_GETTABLE, abc
LOP_SETTABLE, abc
LOP_GETTABLEKS, abc aux
LOP_SETTABLEKS, abc aux
LOP_GETTABLEN, abc
LOP_SETTABLEN, abc
LOP_NEWCLOSURE, ad
LOP_NAMECALL, abc aux
LOP_CALL, abc
LOP_RETURN, ab (c)
LOP_JUMP, (a) d
LOP_JUMPBACK, (a) d
LOP_JUMPIF, ad
LOP_JUMPIFNOT, ad
LOP_JUMPIFEQ, ad aux
LOP_JUMPIFLE, ad aux
LOP_JUMPIFLT, ad aux
LOP_JUMPIFNOTEQ, ad aux
LOP_JUMPIFNOTLE, ad aux
LOP_JUMPIFNOTLT, ad aux
LOP_ADD, abc
LOP_SUB, abc
LOP_MUL, abc
LOP_DIV, abc
LOP_MOD, abc
LOP_POW, abc
LOP_ADDK, abc
LOP_SUBK, abc
LOP_MULK, abc
LOP_DIVK, abc
LOP_MODK, abc
LOP_POWK, abc
LOP_AND, abc
LOP_OR, abc
LOP_ANDK, abc
LOP_ORK, abc
LOP_CONCAT, abc
LOP_NOT, ab (c)
LOP_MINUS, ab (c)
LOP_LENGTH, ab (c)
LOP_NEWTABLE, ab (c) aux
LOP_DUPTABLE, ad
LOP_SETLIST, abc aux
LOP_FORNPREP, ad
LOP_FORNLOOP, ad
LOP_FORGLOOP, ad aux
LOP_FORGPREP_INEXT, ad
LOP_FORGLOOP_INEXT, ad
LOP_FORGPREP_NEXT, ad
LOP_FORGLOOP_NEXT, ad
LOP_GETVARARGS, ab (c)
LOP_DUPCLOSURE, ad
LOP_PREPVARARGS, a (bc)
LOP_LOADKX, a (bc) aux
LOP_JUMPX, e
LOP_FASTCALL, a (b) c
LOP_COVERAGE, e
LOP_CAPTURE, ab (c)
LOP_JUMPIFEQK, ad aux
LOP_JUMPIFNOTEQK, ad aux
LOP_FASTCALL1, abc
LOP_FASTCALL2, abc aux
LOP_FASTCALL2K, abc aux
LOP_FORGPREP, ad

LOP_IDIV, abc

store aud mh

*/

#[derive(Debug, Clone, Copy)]
pub enum Instruction {
    BC {
        op_code: OpCode,
        a: u8,
        b: u8,
        c: u8,
        aux: u32,
    },
    AD {
        op_code: OpCode,
        a: u8,
        d: i16,
        aux: u32,
    },
    E {
        op_code: OpCode,
        e: i32,
    },
}

impl Instruction {
    pub fn parse(insn: u32, encode_key: u8) -> Result<Instruction, nom::error::ErrorKind> {
        let op_code = (insn & 0xFF) as u8;
        let op_code = op_code.wrapping_mul(encode_key);
        match op_code {
            0
            | 1
            | 2
            | 3
            | 6..=11
            | 13..=18
            | 20..=22
            | 33..=53
            | 55
            | 60
            | 63
            | 65
            | 66
            | 68
            | 70
            | 71..=75
            | 81..=85 => {
                let (a, b, c) = Self::parse_abc(insn);

                Ok(Self::BC {
                    op_code: OpCode::try_from(op_code).map_err(|_| nom::error::ErrorKind::Tag)?,
                    a,
                    b,
                    c,
                    aux: 0,
                })
            }
            4 | 5 | 12 | 19 | 23..=32 | 54 | 56..=59 | 61 | 62 | 64 | 76..=80 => {
                let (a, d) = Self::parse_ad(insn);

                Ok(Self::AD {
                    op_code: OpCode::try_from(op_code).map_err(|_| nom::error::ErrorKind::Tag)?,
                    a,
                    d,
                    aux: 0,
                })
            }
            67 | 69 => {
                let e = Self::parse_e(insn);

                Ok(Self::E {
                    op_code: OpCode::try_from(op_code).map_err(|_| nom::error::ErrorKind::Tag)?,
                    e,
                })
            }
            97 => Ok(Self::BC {
                op_code: OpCode::LOP_NOP,
                a: 0,
                b: 0,
                c: 0,
                aux: 0,
            }),
            _ => Err(nom::error::ErrorKind::Tag),
        }
    }

    fn parse_abc(insn: u32) -> (u8, u8, u8) {
        let a = ((insn >> 8) & 0xFF) as u8;
        let b = ((insn >> 16) & 0xFF) as u8;
        let c = ((insn >> 24) & 0xFF) as u8;

        (a, b, c)
    }

    fn parse_ad(insn: u32) -> (u8, i16) {
        let a = ((insn >> 8) & 0xFF) as u8;
        let d = ((insn >> 16) & 0xFFFF) as i16;

        (a, d)
    }

    fn parse_e(insn: u32) -> i32 {
        (insn as i32) >> 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded_opcode(op_code: u8, encode_key: u8) -> u8 {
        (0..=u8::MAX)
            .find(|candidate| candidate.wrapping_mul(encode_key) == op_code)
            .expect("encode key must be invertible modulo 256")
    }

    fn abc_instruction(op_code: u8, encode_key: u8, a: u8, b: u8, c: u8) -> u32 {
        u32::from(encoded_opcode(op_code, encode_key))
            | (u32::from(a) << 8)
            | (u32::from(b) << 16)
            | (u32::from(c) << 24)
    }

    #[test]
    fn parses_version_nine_userdata_opcodes_with_roblox_key() {
        let encode_key = 203;
        for (raw_opcode, expected) in [
            (83, OpCode::LOP_GETUDATAKS),
            (84, OpCode::LOP_SETUDATAKS),
            (85, OpCode::LOP_NAMECALLUDATA),
        ] {
            let instruction =
                Instruction::parse(abc_instruction(raw_opcode, encode_key, 1, 2, 3), encode_key)
                    .unwrap();

            assert!(matches!(
                instruction,
                Instruction::BC {
                    op_code,
                    a: 1,
                    b: 2,
                    c: 3,
                    ..
                } if op_code == expected
            ));
        }
    }

    #[test]
    fn rejects_unknown_opcodes_without_panicking() {
        let result = std::panic::catch_unwind(|| Instruction::parse(86, 1));

        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }
}
