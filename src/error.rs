use std::error::Error;
pub type PrevaylerResult<T> = Result<T, PrevaylerError>;

#[derive(Debug)]
pub enum PrevaylerError {
    IOError(Box<dyn Error>),
    SerializationError(crate::serializer::SerializerError),
}

impl From<std::io::Error> for PrevaylerError {
    fn from(error: std::io::Error) -> Self {
        PrevaylerError::IOError(Box::new(error))
    }
}

impl From<crate::serializer::SerializerError> for PrevaylerError {
    fn from(error: crate::serializer::SerializerError) -> Self {
        PrevaylerError::SerializationError(error)
    }
}
