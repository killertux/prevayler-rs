use pin_project_lite::pin_project;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::error::Error;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;
use tokio::fs::{File, OpenOptions, read_dir};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_stream::{Stream, StreamExt};

pub struct PrevaylerBuilder<Transactions, Data, LogSerializer = JsonSerializer> {
    path: Option<PathBuf>,
    serializer: Option<LogSerializer>,
    log_limit: Option<usize>,
    data: Data,
    _transactions: PhantomData<Transactions>,
}

impl<Transactions, Data, LogSerializer> PrevaylerBuilder<Transactions, Data, LogSerializer> {
    pub fn create(data: Data) -> Self {
        Self {
            path: None,
            serializer: None,
            log_limit: None,
            data,
            _transactions: PhantomData {},
        }
    }

    pub fn with_path(mut self, path: impl AsRef<Path>) -> Self {
        self.path = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn with_serializer(mut self, serializer: LogSerializer) -> Self
    where
        LogSerializer: Serializer<Transactions>,
    {
        self.serializer = Some(serializer);
        self
    }

    pub async fn build(self) -> Result<Prevayler<Data, Transactions, LogSerializer>, PrevaylerError>
    where
        Transactions: Transaction<Data>,
        LogSerializer: Serializer<Transactions> + Default,
    {
        Prevayler::new(
            self.data,
            self.path.unwrap_or(".".into()),
            self.serializer.unwrap_or_default(),
        )
        .await
    }
}

#[derive(Error, Debug)]
pub enum PrevaylerError {
    #[error("Unexpected log file")]
    UnexpectedLogFile,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error("Serialization error `{0}`")]
    SerializationError(Box<dyn Error + Send + Sync + 'static>),
    #[error("Deserialization erro `{0}`")]
    DeserializationError(Box<dyn Error + Send + Sync + 'static>),
}

impl From<RedoLogError> for PrevaylerError {
    fn from(error: RedoLogError) -> Self {
        match error {
            RedoLogError::UnexpectedLogFile => PrevaylerError::UnexpectedLogFile,
            RedoLogError::IoError(err) => PrevaylerError::IoError(err),
            RedoLogError::SerializeError(err) => PrevaylerError::DeserializationError(err),
            RedoLogError::DeserializeError(err) => PrevaylerError::SerializationError(err),
        }
    }
}

pub struct Prevayler<Data, Transactions, LogSerializer> {
    data: Data,
    redo_log: RedoLog,
    serializer: LogSerializer,
    path: PathBuf,
    _t: PhantomData<Transactions>,
}

impl<Data, Transactions, LogSerializer> Prevayler<Data, Transactions, LogSerializer>
where
    LogSerializer: Serializer<Transactions>,
    Transactions: Transaction<Data>,
{
    async fn new(
        mut data: Data,
        path: impl AsRef<Path>,
        mut serializer: LogSerializer,
    ) -> Result<Self, PrevaylerError> {
        let path = path.as_ref().to_owned();
        let redo_log = RedoLog::new(&mut data, &path, &mut serializer).await?;
        Ok(Prevayler {
            data,
            redo_log,
            serializer,
            path,
            _t: PhantomData {},
        })
    }

    pub async fn execute(
        &mut self,
        transaction: impl Into<Transactions>,
    ) -> Result<Transactions::Output, PrevaylerError> {
        let transaction = transaction.into();
        self.redo_log
            .write_transaction_log(&mut self.serializer, &transaction)
            .await?;
        Ok(transaction.execute(&mut self.data))
    }

    pub async fn execute_with_snapshot(
        &mut self,
        transaction: impl Into<Transactions>,
        limit: usize,
    ) -> Result<Transactions::Output, PrevaylerError> {
        let transaction = transaction.into();
        let n_transactions = self
            .redo_log
            .write_transaction_log(&mut self.serializer, &transaction)
            .await?;
        if n_transactions >= limit {
            
        }
        Ok(transaction.execute(&mut self.data))
    }

    pub fn query(&self) -> &Data {
        &self.data
    }
}

#[derive(Error, Debug)]
pub enum RedoLogError {
    #[error("Unexpected redo log file found")]
    UnexpectedLogFile,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    SerializeError(Box<dyn Error + Send + Sync + 'static>),
    #[error(transparent)]
    DeserializeError(Box<dyn Error + Send + Sync + 'static>),
}

struct LogFile {
    file: File,
    file_index: usize,
    n_transactions: usize,
}

pub struct RedoLog {
    file: LogFile,
    path: PathBuf,
}

impl RedoLog {
    pub async fn new<Target, Transactions, LogSerializer>(
        data: &mut Target,
        path: impl AsRef<Path>,
        serializer: &mut LogSerializer,
    ) -> Result<Self, RedoLogError>
    where
        LogSerializer: Serializer<Transactions>,
        Transactions: Transaction<Target>,
    {
        let path = path.as_ref();
        let mut files = read_log_files(&path).await?;
        let mut n_transactions = 0;
        for file in files.iter() {
            n_transactions = 0;
            let handle = File::open(file).await?;
            let mut stream = serializer.desserialize(handle);
            while let Some(transaction) = stream.next().await {
                let transaction =
                    transaction.map_err(|err| RedoLogError::DeserializeError(Box::new(err)))?;
                let _ = transaction.execute(data);
                n_transactions += 1;
            }
        }
        let last_file_to_open = files.pop().unwrap_or(path.join("redo_log.00000000.log"));
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&last_file_to_open)
            .await?;

        Ok(Self {
            file: LogFile {
                file,
                file_index: last_file_to_open
                    .file_name()
                    .ok_or(RedoLogError::UnexpectedLogFile)?
                    .to_string_lossy()
                    .split_terminator('.')
                    .skip(1)
                    .take(1)
                    .map(|index| index.parse::<usize>())
                    .collect::<Result<Vec<usize>, core::num::ParseIntError>>()
                    .map_err(|_| RedoLogError::UnexpectedLogFile)?[0],
                n_transactions,
            },
            path: path.to_path_buf(),
        })
    }

    pub async fn write_transaction_log<T, LogSerializer>(
        &mut self,
        serializer: &mut LogSerializer,
        transaction: &T,
    ) -> Result<usize, RedoLogError>
    where
        LogSerializer: Serializer<T>,
    {
        serializer
            .serialize(&mut self.file.file, transaction)
            .await
            .map_err(|err| RedoLogError::SerializeError(Box::new(err)))?;
        self.file.file.sync_all().await?;
        self.file.n_transactions += 1;
        // if Some(self.file.n_transactions) == self.log_limit {
        //     let new_file_index = self.file.file_index + 1;
        //     let mut path = self.path.clone();
        //     path.push(format!("redo_log.{:0>8}.log", new_file_index));
        //     self.file = LogFile {
        //         file: File::create(path).await?,
        //         file_index: new_file_index,
        //         n_transactions: 0,
        //     }
        // }
        Ok(self.file.n_transactions)
    }
}

async fn read_log_files(path: impl AsRef<Path>) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut dir = read_dir(path).await?;
    let mut result = Vec::new();
    while let Some(entry) = dir.next_entry().await? {
        if !entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".log")
        {
            continue;
        }
        if !entry.file_type().await?.is_file() {
            continue;
        }
        result.push(entry.path());
    }
    result.sort();
    Ok(result)
}

pub trait Serializer<T> {
    type SerializerError: Error + Send + Sync + 'static;
    type DeserializeError: Error + Send + Sync + 'static;
    type TransactionsStream<R>: Stream<Item = Result<T, Self::DeserializeError>> + Unpin
    where
        R: AsyncRead + Unpin;
    fn desserialize<R>(&self, read: R) -> Self::TransactionsStream<R>
    where
        R: AsyncRead + Unpin;
    fn serialize<W>(
        &self,
        writer: &mut W,
        value: &T,
    ) -> impl Future<Output = Result<(), Self::SerializerError>>
    where
        W: AsyncWrite + Unpin + Send;
}

pub trait Transaction<Target> {
    type Output;
    fn execute(self, target: &mut Target) -> Self::Output;
}

#[derive(Error, Debug)]
pub enum JsonSerializeError {
    #[error(transparent)]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    SerdeJsonError(#[from] serde_json::Error),
}
#[derive(Default)]
pub struct JsonSerializer {}

impl<T> Serializer<T> for JsonSerializer
where
    T: Serialize + DeserializeOwned + Sync + Unpin,
{
    type SerializerError = JsonSerializeError;
    type DeserializeError = JsonSerializeError;
    type TransactionsStream<R>
        = JsonSerializerStream<R, T>
    where
        R: AsyncRead + Unpin;
    fn desserialize<R>(&self, read: R) -> Self::TransactionsStream<R>
    where
        R: AsyncRead + Unpin,
    {
        JsonSerializerStream {
            read,
            buffer: Vec::new(),
            _t: PhantomData {},
        }
    }

    async fn serialize<W>(&self, writer: &mut W, value: &T) -> Result<(), Self::SerializerError>
    where
        W: AsyncWrite + Unpin + Send,
    {
        let mut ser = serde_json::to_vec(value)?;
        ser.push(b'\n');
        writer.write_all(&ser).await?;
        Ok(())
    }
}

pin_project! {
    pub struct JsonSerializerStream<R, T> {
        #[pin]
        read: R,
        buffer: Vec<u8>,
        _t: PhantomData<T>,
    }
}

impl<R, T> Stream for JsonSerializerStream<R, T>
where
    R: AsyncRead,
    T: DeserializeOwned,
{
    type Item = Result<T, JsonSerializeError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        let mut read = this.read;
        let buffer = this.buffer;

        loop {
            match buffer.iter().position(|c| *c == b'\n') {
                Some(n) => {
                    let (head, tail) = buffer.split_at(n + 1);
                    let transaction: T = serde_json::de::from_slice(head)?;
                    let tail = tail.into();
                    *buffer = tail;
                    return Poll::Ready(Some(Ok(transaction)));
                }
                None => {
                    let mut temp = [0; 1 << 16];
                    let mut new_buffer = ReadBuf::new(&mut temp);
                    match read.as_mut().poll_read(cx, &mut new_buffer) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Ok(())) if new_buffer.filled().is_empty() => {
                            return Poll::Ready(None);
                        }
                        Poll::Ready(Ok(())) => {
                            buffer.extend_from_slice(new_buffer.filled());
                            continue;
                        }
                        Poll::Ready(Err(err)) => {
                            return Poll::Ready(Some(Err(JsonSerializeError::IoError(err))));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use serde::Deserialize;
    use temp_testdir::TempDir;

    use super::*;

    #[derive(Serialize, Deserialize)]
    struct PlusN(i64);

    impl Transaction<i64> for PlusN {
        type Output = i64;
        fn execute(self, target: &mut i64) -> Self::Output {
            *target += self.0;
            *target
        }
    }

    #[tokio::test]
    async fn simple_transaction() -> anyhow::Result<()> {
        let tempdir = TempDir::default();
        let mut prevailer: Prevayler<_, PlusN, JsonSerializer> = PrevaylerBuilder::create(10)
            .with_path(&tempdir)
            .build()
            .await?;
        let plus_2 = PlusN(2);
        let result = prevailer.execute(plus_2).await?;
        assert_eq!(12, result);
        assert_eq!(12, *prevailer.query());
        Ok(())
    }

    #[tokio::test]
    async fn simple_transaction_with_persistence() -> anyhow::Result<()> {
        let tempdir = TempDir::default();
        let mut prevailer: Prevayler<i64, PlusN, _> =
            Prevayler::new(10, &tempdir, JsonSerializer {}).await?;
        let plus_2 = PlusN(2);
        let result = prevailer.execute(plus_2).await?;
        let prevailer: Prevayler<i64, PlusN, _> =
            Prevayler::new(10, &tempdir, JsonSerializer {}).await?;
        assert_eq!(12, result);
        assert_eq!(12, *prevailer.query());
        Ok(())
    }

    #[tokio::test]
    async fn multiple_log_files() -> anyhow::Result<()> {
        let tempdir = TempDir::default();
        let mut prevailer: Prevayler<i64, PlusN, _> =
            Prevayler::new(10, &tempdir, JsonSerializer {}).await?;
        prevailer.execute(PlusN(2)).await?;
        prevailer.execute(PlusN(2)).await?;
        prevailer.execute(PlusN(2)).await?;
        prevailer.execute(PlusN(2)).await?;
        prevailer.execute(PlusN(2)).await?;
        let prevailer: Prevayler<i64, PlusN, _> =
            Prevayler::new(10, &tempdir, JsonSerializer {}).await?;
        assert_eq!(20, *prevailer.query());
        let log_files: Vec<String> = read_log_files(&tempdir)
            .await?
            .into_iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            vec![
                "redo_log.00000000.log".to_string(),
                "redo_log.00000001.log".to_string(),
                "redo_log.00000002.log".to_string()
            ],
            log_files
        );
        Ok(())
    }

    // use serde::{Deserialize, Serialize};
    // use std::sync::Arc;
    // use std::thread;
    // use temp_testdir::TempDir;

    // #[derive(Serialize, Deserialize)]
    // struct ChangeFirstElement {
    //     value: u8,
    // }

    // #[derive(Serialize, Deserialize)]
    // struct ChangeSecondElement {
    //     value: u8,
    // }

    // #[derive(Serialize, Deserialize)]
    // struct AddToFirstElement {
    //     value: u8,
    // }

    // #[derive(Serialize, Deserialize)]
    // struct AddToSecondElementFailling {
    //     value: u8,
    // }

    // #[derive(Serialize, Deserialize)]
    // struct AddToSecondElement {
    //     value: u8,
    // }

    // impl Transaction<(u8, u8)> for ChangeFirstElement {
    //     fn execute(self, data: &mut (u8, u8)) {
    //         data.0 = self.value;
    //     }
    // }

    // impl Transaction<(u8, u8)> for ChangeSecondElement {
    //     fn execute(self, data: &mut (u8, u8)) {
    //         data.1 = self.value;
    //     }
    // }

    // impl Transaction<(u8, u8)> for AddToFirstElement {
    //     fn execute(self, data: &mut (u8, u8)) {
    //         data.0 += self.value;
    //     }
    // }

    // impl Transaction<(u8, u8)> for AddToSecondElementFailling {
    //     fn execute(self, data: &mut (u8, u8)) {
    //         data.0 += self.value;
    //         panic!("Fail");
    //     }
    // }

    // impl TransactionWithQuery<(u8, u8)> for AddToSecondElement {
    //     type Output = u8;
    //     fn execute_and_return(&self, data: &mut (u8, u8)) -> u8 {
    //         let old_value = data.0;
    //         data.0 += self.value;
    //         return old_value;
    //     }
    // }

    // #[derive(Serialize, Deserialize)]
    // enum Transactions {
    //     ChangeFirstElement(ChangeFirstElement),
    //     ChangeSecondElement(ChangeSecondElement),
    //     AddToFirstElement(AddToFirstElement),
    //     AddToSecondElementFailling(AddToSecondElementFailling),
    //     AddToSecondElement(AddToSecondElement),
    // }

    // impl Transaction<(u8, u8)> for Transactions {
    //     fn execute(self, data: &mut (u8, u8)) {
    //         match self {
    //             Transactions::ChangeFirstElement(e) => {
    //                 e.execute(data);
    //             }
    //             Transactions::ChangeSecondElement(e) => {
    //                 e.execute(data);
    //             }
    //             Transactions::AddToFirstElement(e) => {
    //                 e.execute(data);
    //             }
    //             Transactions::AddToSecondElementFailling(e) => {
    //                 e.execute(data);
    //             }
    //             Transactions::AddToSecondElement(e) => {
    //                 e.execute(data);
    //             }
    //         };
    //     }
    // }

    // impl Into<Transactions> for ChangeFirstElement {
    //     fn into(self) -> Transactions {
    //         Transactions::ChangeFirstElement(self)
    //     }
    // }

    // impl Into<Transactions> for ChangeSecondElement {
    //     fn into(self) -> Transactions {
    //         Transactions::ChangeSecondElement(self)
    //     }
    // }

    // impl Into<Transactions> for AddToFirstElement {
    //     fn into(self) -> Transactions {
    //         Transactions::AddToFirstElement(self)
    //     }
    // }

    // impl Into<Transactions> for AddToSecondElementFailling {
    //     fn into(self) -> Transactions {
    //         Transactions::AddToSecondElementFailling(self)
    //     }
    // }

    // impl Into<Transactions> for AddToSecondElement {
    //     fn into(self) -> Transactions {
    //         Transactions::AddToSecondElement(self)
    //     }
    // }

    // #[async_std::test]
    // async fn test_transaction() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     let data = (3, 4);
    //     let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //         .path(&temp.as_os_str())
    //         .max_log_size(10)
    //         .serializer(JsonSerializer::new())
    //         .data(data)
    //         .build()
    //         .await?;
    //     prevayler
    //         .execute_transaction(ChangeFirstElement { value: 7 })
    //         .await?;
    //     prevayler
    //         .execute_transaction(ChangeSecondElement { value: 32 })
    //         .await?;
    //     assert_eq!(&(7, 32), prevayler.query());
    //     Ok(())
    // }

    // #[async_std::test]
    // async fn test_multi_threading() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     let data = (3, 4);
    //     let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //         .path(&temp.as_os_str())
    //         .max_log_size(10)
    //         .serializer(JsonSerializer::new())
    //         .data(data)
    //         .build()
    //         .await?;
    //     let prevayler = Arc::new(Mutex::new(prevayler));

    //     let prevayler_clone = prevayler.clone();
    //     let handle_1 = thread::spawn(move || {
    //         async_std::task::block_on(async {
    //             let mut guard = prevayler_clone.lock().await;
    //             guard
    //                 .execute_transaction(ChangeFirstElement { value: 7 })
    //                 .await
    //                 .expect("Error executing transaction")
    //         });
    //     });
    //     let prevayler_clone = prevayler.clone();
    //     let handle_2 = thread::spawn(move || {
    //         async_std::task::block_on(async {
    //             let mut guard = prevayler_clone.lock().await;
    //             guard
    //                 .execute_transaction(ChangeSecondElement { value: 32 })
    //                 .await
    //                 .expect("Error executing transaction")
    //         });
    //     });
    //     handle_1.join().unwrap();
    //     handle_2.join().unwrap();

    //     let guard = prevayler.lock().await;
    //     let query = guard.query();
    //     assert_eq!(7, query.0);
    //     assert_eq!(32, query.1);
    //     Ok(())
    // }

    // #[async_std::test]
    // async fn test_panic_in_execute_transaction_panic_safe() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     let data = (3, 4);
    //     let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //         .path(&temp.as_os_str())
    //         .max_log_size(10)
    //         .serializer(JsonSerializer::new())
    //         .data(data)
    //         .build()
    //         .await?;
    //     let prevayler = Arc::new(Mutex::new(prevayler));

    //     let prevayler_clone = prevayler.clone();
    //     let handle_1 = thread::spawn(move || {
    //         async_std::task::block_on(async {
    //             let mut guard = prevayler_clone.lock().await;
    //             guard
    //                 .execute_transaction(ChangeFirstElement { value: 7 })
    //                 .await
    //                 .expect("Error executing transaction")
    //         });
    //     });
    //     let prevayler_clone = prevayler.clone();
    //     let handle_2 = thread::spawn(move || {
    //         async_std::task::block_on(async {
    //             let mut guard = prevayler_clone.lock().await;
    //             guard
    //                 .execute_transaction_panic_safe(AddToSecondElementFailling { value: 32 })
    //                 .await
    //                 .expect("Error executing transaction")
    //         });
    //     });
    //     handle_1.join().unwrap();
    //     assert_eq!(true, handle_2.join().is_err());

    //     let guard = prevayler.lock().await;
    //     let query = guard.query();
    //     assert_eq!(7, query.0);
    //     assert_eq!(4, query.1);
    //     Ok(())
    // }

    // #[async_std::test]
    // async fn test_should_save_state() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     {
    //         let data = (3, 4);
    //         let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build()
    //             .await?;
    //         prevayler
    //             .execute_transaction(ChangeFirstElement { value: 7 })
    //             .await?;
    //     }
    //     {
    //         let data = (3, 4);
    //         let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build()
    //             .await?;
    //         prevayler
    //             .execute_transaction(ChangeSecondElement { value: 32 })
    //             .await?;
    //     }
    //     {
    //         let data = (3, 4);
    //         let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build()
    //             .await?;
    //         assert_eq!(&(7, 32), prevayler.query());
    //     }
    //     Ok(())
    // }

    // #[async_std::test]
    // async fn test_redo_log_with_snapshot() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     {
    //         let data = (3, 4);
    //         let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build_with_snapshots()
    //             .await?;
    //         prevayler
    //             .execute_transaction(AddToFirstElement { value: 7 })
    //             .await?;
    //         prevayler.snapshot().await?;
    //     }
    //     {
    //         let data = (3, 4);
    //         let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build_with_snapshots()
    //             .await?;
    //         prevayler
    //             .execute_transaction(AddToFirstElement { value: 1 })
    //             .await?;
    //         assert_eq!(&(11, 4), prevayler.query());
    //     }
    //     {
    //         let data = (0, 0);
    //         let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //             .path(&temp.as_os_str())
    //             .max_log_size(10)
    //             .serializer(JsonSerializer::new())
    //             .data(data)
    //             .build_with_snapshots()
    //             .await?;
    //         assert_eq!(&(11, 4), prevayler.query());
    //     }
    //     Ok(())
    // }

    // #[async_std::test]
    // async fn test_transaction_with_query() -> PrevaylerResult<()> {
    //     let temp = TempDir::default();
    //     let data = (3, 4);
    //     let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
    //         .path(&temp.as_os_str())
    //         .max_log_size(10)
    //         .serializer(JsonSerializer::new())
    //         .data(data)
    //         .build()
    //         .await?;

    //     assert_eq!(
    //         3,
    //         prevayler
    //             .execute_transaction_with_query(AddToSecondElement { value: 7 })
    //             .await?
    //     );
    //     assert_eq!(
    //         10,
    //         prevayler
    //             .execute_transaction_with_query(AddToSecondElement { value: 5 })
    //             .await?
    //     );
    //     assert_eq!(&(15, 4), prevayler.query());
    //     Ok(())
    // }
}
