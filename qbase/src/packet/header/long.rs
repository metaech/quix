use super::*;
use crate::{cid::ConnectionId, varint::VarInt};
use deref_derive::{Deref, DerefMut};
use nom::ToUsize;

// The following is the unencrypted header content, which may exist in all future versions
// of QUIC, so it is placed in this file without distinguishing versions.

#[derive(Debug, Default, Clone)]
pub struct VersionNegotiation {
    pub versions: Vec<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct Retry {
    pub token: Vec<u8>,
    pub integrity: [u8; 16],
}

impl Retry {
    fn from_slice(token: &[u8], integrity: &[u8]) -> Self {
        let mut retry = Retry {
            token: Vec::from(token),
            integrity: [0; 16],
        };
        retry.integrity.copy_from_slice(integrity);
        retry
    }
}

#[derive(Debug, Default, Clone)]
pub struct Initial {
    pub token: Vec<u8>,
    pub length: VarInt,
}

#[derive(Debug, Default, Clone)]
pub struct ZeroRtt {
    pub length: VarInt,
}

#[derive(Debug, Default, Clone)]
pub struct Handshake {
    pub length: VarInt,
}

macro_rules! protect {
    ($($type:ty),*) => {
        $(
            impl super::Protect for $type {}

            impl super::GetLength for $type {
                fn get_length(&self) -> usize {
                    self.length.to_usize()
                }
            }
        )*
    };
}

protect!(Initial, ZeroRtt, Handshake);

#[derive(Debug, Default, Clone, Deref, DerefMut)]
pub struct LongHeader<T> {
    pub dcid: ConnectionId,
    pub scid: ConnectionId,
    #[deref]
    pub specific: T,
}

impl<T> super::GetDcid for LongHeader<T> {
    fn get_dcid(&self) -> &ConnectionId {
        &self.dcid
    }
}

pub type VersionNegotiationHeader = LongHeader<VersionNegotiation>;
pub type RetryHeader = LongHeader<Retry>;

pub type InitialHeader = LongHeader<Initial>;
pub type HandshakeHeader = LongHeader<Handshake>;
pub type ZeroRttHeader = LongHeader<ZeroRtt>;

impl Protect for InitialHeader {}
impl Protect for ZeroRttHeader {}
impl Protect for HandshakeHeader {}

macro_rules! bind_type {
    ($($type:ty => $value:expr),*) => {
        $(
            impl GetType for $type {
                fn get_type(&self) -> Type {
                    $value
                }
            }
        )*
    };
}

bind_type!(
    VersionNegotiationHeader => Type::Long(LongType::VersionNegotiation),
    RetryHeader => Type::Long(LongType::V1(Version::<1, _>(v1::Type::Retry))),
    InitialHeader => Type::Long(LongType::V1(Version::<1, _>(v1::Type::Initial))),
    ZeroRttHeader => Type::Long(LongType::V1(Version::<1, _>(v1::Type::ZeroRtt))),
    HandshakeHeader => Type::Long(LongType::V1(Version::<1, _>(v1::Type::Handshake)))
);

pub(super) mod ext {
    use std::ops::Deref;

    use super::*;
    use crate::{
        cid::WriteConnectionId,
        packet::r#type::long::{v1::Type as LongV1Type, Type as LongType},
        varint::ext::{be_varint, BufMutExt},
    };
    use bytes::BufMut;
    use nom::{
        bytes::streaming::take,
        combinator::{eof, map},
        multi::{length_data, many_till},
        number::streaming::be_u32,
        sequence::pair,
        Err,
    };

    pub fn be_version_negotiation(input: &[u8]) -> nom::IResult<&[u8], VersionNegotiation> {
        let (remain, (versions, _)) = many_till(be_u32, eof)(input)?;
        Ok((remain, VersionNegotiation { versions }))
    }

    pub fn be_initial(input: &[u8]) -> nom::IResult<&[u8], Initial> {
        map(
            pair(length_data(be_varint), be_varint),
            |(token, length)| Initial {
                token: Vec::from(token),
                length,
            },
        )(input)
    }

    pub fn be_zero_rtt(input: &[u8]) -> nom::IResult<&[u8], ZeroRtt> {
        map(be_varint, |length| ZeroRtt { length })(input)
    }

    pub fn be_handshake(input: &[u8]) -> nom::IResult<&[u8], Handshake> {
        map(be_varint, |length| Handshake { length })(input)
    }

    pub fn be_retry(input: &[u8]) -> nom::IResult<&[u8], Retry> {
        if input.len() < 16 {
            return Err(Err::Incomplete(nom::Needed::new(16)));
        }
        let token_length = input.len() - 16;
        let (integrity, token) = take(token_length)(input)?;
        Ok((&[][..], Retry::from_slice(token, integrity)))
    }

    pub struct LongHeaderBuilder {
        pub(crate) dcid: ConnectionId,
        pub(crate) scid: ConnectionId,
    }

    impl LongHeaderBuilder {
        pub fn with_cid(dcid: ConnectionId, scid: ConnectionId) -> Self {
            Self { dcid, scid }
        }

        pub fn wrap<T>(self, specific: T) -> LongHeader<T> {
            LongHeader {
                dcid: self.dcid,
                scid: self.scid,
                specific,
            }
        }

        pub fn parse(self, ty: LongType, input: &[u8]) -> nom::IResult<&[u8], Header> {
            match ty {
                LongType::VersionNegotiation => {
                    let (remain, versions) = be_version_negotiation(input)?;
                    Ok((remain, Header::VN(self.wrap(versions))))
                }
                LongType::V1(ty) => match ty.deref() {
                    LongV1Type::Retry => {
                        let (remain, retry) = be_retry(input)?;
                        Ok((remain, Header::Retry(self.wrap(retry))))
                    }
                    LongV1Type::Initial => {
                        let (remain, initial) = be_initial(input)?;
                        Ok((remain, Header::Initial(self.wrap(initial))))
                    }
                    LongV1Type::ZeroRtt => {
                        let (remain, zero_rtt) = be_zero_rtt(input)?;
                        Ok((remain, Header::ZeroRtt(self.wrap(zero_rtt))))
                    }
                    LongV1Type::Handshake => {
                        let (remain, handshake) = be_handshake(input)?;
                        Ok((remain, Header::Handshake(self.wrap(handshake))))
                    }
                },
            }
        }
    }

    trait Write<S> {
        fn put_specific(&mut self, specific: &S);
    }

    impl<T: BufMut> Write<VersionNegotiation> for T {
        fn put_specific(&mut self, specific: &VersionNegotiation) {
            for version in &specific.versions {
                self.put_u32(*version);
            }
        }
    }

    impl<T: BufMut> Write<Retry> for T {
        fn put_specific(&mut self, specific: &Retry) {
            self.put_slice(&specific.integrity);
            self.put_slice(&specific.token);
        }
    }

    impl<T: BufMut> Write<Initial> for T {
        fn put_specific(&mut self, specific: &Initial) {
            self.put_varint(&VarInt(specific.token.len() as u64));
            self.put_slice(&specific.token);
            self.put_varint(&specific.length);
        }
    }

    impl<T: BufMut> Write<ZeroRtt> for T {
        fn put_specific(&mut self, specific: &ZeroRtt) {
            self.put_varint(&specific.length);
        }
    }

    impl<T: BufMut> Write<Handshake> for T {
        fn put_specific(&mut self, specific: &Handshake) {
            self.put_varint(&specific.length);
        }
    }

    pub trait WriteLongHeader<T> {
        fn put_long_header(&mut self, wrapper: &LongHeader<T>);
    }

    impl<T, S> WriteLongHeader<S> for T
    where
        T: BufMut + Write<S>,
    {
        fn put_long_header(&mut self, long_header: &LongHeader<S>) {
            self.put_connection_id(&long_header.dcid);
            self.put_connection_id(&long_header.scid);
            self.put_specific(&long_header.specific);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4)
    }
}
