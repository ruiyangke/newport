//! Standalone compatibility and runtime ownership share the integration loop.
use crate::{
    clipboard_service::{Access, Producer},
    clipboard_source::Data,
    clipboard_wire::Offer,
    demand::Store,
};
use std::{io, sync::Arc};

pub(crate) enum Channel {
    Local(Arc<Store>),
    Runtime(Producer),
}
impl Channel {
    /// Runtime-native selections remain local data when host imports are off.
    pub(crate) fn local_reads(&self) -> bool {
        matches!(self, Self::Runtime(_))
    }
    pub(crate) fn export_reader(&self) -> Option<crate::clipboard_service::Reader> {
        match self {
            Self::Local(_) => None,
            Self::Runtime(producer) => Some(producer.reader()),
        }
    }
    pub(crate) fn new(access: Option<Access>) -> io::Result<Self> {
        access.map_or_else(
            || Ok(Self::Local(Store::new())),
            |access| Producer::new(access).map(Self::Runtime),
        )
    }
    pub(crate) fn enabled(&mut self, enabled: bool) -> io::Result<()> {
        match self {
            Self::Local(store) if !enabled => {
                store.close();
                *store = Store::new();
                Ok(())
            }
            Self::Local(_) => Ok(()),
            Self::Runtime(producer) => producer.enabled(enabled),
        }
    }
    pub(crate) fn data(&self) -> Data {
        match self {
            Self::Local(store) => Data::Local(store.clone()),
            Self::Runtime(producer) => Data::Runtime(producer.reader()),
        }
    }
    pub(crate) fn update(&self, offer: Offer) -> io::Result<()> {
        match self {
            Self::Local(store) => {
                store.update(offer);
                Ok(())
            }
            Self::Runtime(producer) => producer.update(offer),
        }
    }
    pub(crate) fn reply(&self, bytes: &[u8]) -> io::Result<()> {
        match self {
            Self::Local(store) => store.reply(bytes),
            Self::Runtime(producer) => producer.reply(bytes),
        }
    }
    pub(crate) fn poll(&self, output: &mut impl io::Write) -> io::Result<()> {
        match self {
            Self::Local(store) => store.poll(output),
            Self::Runtime(producer) => producer.poll(output),
        }
    }
}
