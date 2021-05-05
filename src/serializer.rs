//! This module handles serialization and deserialization of the redolog and snapshots
//!
//! As a default, it provides a [`JsonSerializer`] that does serialization in json format for anyone that implements the serde serialization traits. But you can disable it and use whaetever format that you want.

use async_std::io::Read;
use async_std::stream::Stream;
use std::error;
use std::marker::Unpin;

#[cfg(feature = "json_serializer")]
pub use json_serializer::JsonSerializer;

pub type SerializerResult<T> = Result<T, SerializerError>;

/// Serializer trait
///
/// This methods defines how to serialize and deserialize transactions and the snapshoted data. You can implement it to have your logs in whaetever format that you like.
pub trait Serializer<T> {
    /// Serialize method. It receives a reference to a data and it should return a boxed byte array with the serialized result.
    ///
    /// The redolog will consist of appending multiple calls to this method. The snapshot will be a single file with the result of one call to this method.
    fn serialize(&self, data_to_serialize: &T) -> SerializerResult<Box<[u8]>>;

    /// Deserialize method. It receives a reader to the redolog (or snapshot), and it should return a stream of deserialized data
    ///
    /// In the case of the redolog, it will execute all transactions returned in the stream. If it is a snaphot, only the first item will be used.
    fn deserialize<'a, R: Read + Unpin + 'a>(
        &self,
        reader: R,
    ) -> Box<dyn Stream<Item = SerializerResult<T>> + Unpin + 'a>
    where
        T: 'a;
}

#[derive(Debug)]
pub enum SerializerError {
    ErrorSerializing(Box<dyn error::Error + Send>),
    ErrorDeserializing(Box<dyn error::Error + Send>),
}

#[cfg(feature = "json_serializer")]
pub mod json_serializer {
    //! Json serializer. Only available if using "json_serializer" or "default" features.

    use super::*;
    use core::pin::Pin;
    use futures::task::{Context, Poll};

    /// Implementation of [`Serializer`] for the Json format.
    ///
    /// This implements the [`Serializer`] trait for every data that also implements serde [Serialize](serde::Serialize) and [Deserialize](serde::de::DeserializeOwned).
    pub struct JsonSerializer {}

    impl JsonSerializer {
        pub fn new() -> Self {
            JsonSerializer {}
        }
    }

    impl<T> Serializer<T> for JsonSerializer
    where
        T: serde::Serialize + serde::de::DeserializeOwned + Unpin,
    {
        fn serialize(&self, data_to_serialize: &T) -> SerializerResult<Box<[u8]>> {
            let mut ser = serde_json::to_vec(&data_to_serialize)
                .map_err(|err| SerializerError::ErrorSerializing(Box::new(err)))?;
            ser.push(b'\n');
            Ok(ser.into())
        }

        fn deserialize<'a, R: Read + Unpin + 'a>(
            &self,
            reader: R,
        ) -> Box<dyn Stream<Item = SerializerResult<T>> + Unpin + 'a>
        where
            T: 'a,
        {
            Box::new(JsonDeserializerStream::new(reader))
        }
    }

    struct JsonDeserializerStream<R, T> {
        reader: R,
        buf: Vec<u8>,
        _p: std::marker::PhantomData<T>,
    }

    impl<R, T> JsonDeserializerStream<R, T>
    where
        R: Read,
    {
        fn new(reader: R) -> Self {
            JsonDeserializerStream {
                reader,
                buf: Vec::new(),
                _p: std::marker::PhantomData {},
            }
        }

        fn process_buffer(&mut self) -> Option<SerializerResult<T>>
        where
            T: serde::de::DeserializeOwned,
        {
            for (index, byte) in self.buf.iter().enumerate() {
                if *byte == b'\n' {
                    let data = serde_json::from_slice(&self.buf[..index])
                        .map_err(|err| SerializerError::ErrorDeserializing(Box::new(err)));
                    self.buf = self.buf.split_off(index + 1);
                    return Some(data);
                }
            }
            return None;
        }
    }

    impl<R, T> futures::stream::Stream for JsonDeserializerStream<R, T>
    where
        R: Read + Unpin,
        T: serde::de::DeserializeOwned + Unpin,
    {
        type Item = SerializerResult<T>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = &mut self.get_mut();
            if let Some(data) = this.process_buffer() {
                return Poll::Ready(Some(data));
            }
            loop {
                let mut buf = [0; 1024];
                return match Pin::new(&mut this.reader).poll_read(cx, &mut buf) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(read_bytes) => {
                        let read_bytes = read_bytes.expect("Error");
                        if read_bytes == 0 {
                            return Poll::Ready(None);
                        }
                        this.buf.append(&mut Vec::from(&buf[..read_bytes]));
                        if let Some(data) = this.process_buffer() {
                            return Poll::Ready(Some(data));
                        }
                        continue;
                    }
                };
            }
        }
    }
}
