//!  # Prevayler-rs
//! 
//! This is a simple implementation of a system prevalance proposed by Klaus Wuestefeld in Rust.
//! Other examples are the [prevayler](https://prevayler.org/) for Java and [prevayler-clj](https://github.com/klauswuestefeld/prevayler-clj) for Clojure.
//! 
//! The idea is to save in a redolog all modifications to the prevailed data. If the system restarts, it will re-apply all transactions from the redolog restoring the system state. The system may also write snapshots from time to time to speed-up the recover process.
//! 
//! Here is an example of a program that creates a prevailed state using an u8, increments it, print the value to the screen and closes the program.
//! 
//! ```rust
//! use prevayler_rs::{
//!     error::PrevaylerResult,
//!     PrevaylerBuilder,
//!     Prevayler,
//!     serializer::JsonSerializer,
//!     Transaction
//! };
//! use serde::{Deserialize, Serialize};
//! 
//! #[derive(Serialize, Deserialize)]
//! struct Increment {
//!     increment: u8
//! }
//! 
//! impl Transaction<u8> for Increment {
//!     fn execute(self, data: &mut u8) {
//!         *data += self.increment;
//!     }
//! }
//! 
//! #[async_std::main]
//! async fn main() -> PrevaylerResult<()> {
//!     let mut prevayler: Prevayler<Increment, _, _> = PrevaylerBuilder::new()
//!       .path(".")
//!       .serializer(JsonSerializer::new())
//!       .data(0 as u8)
//!       .build().await?;
//!     prevayler.execute_transaction(Increment{increment: 1}).await?;
//!     println!("{}", prevayler.query());
//!     Ok(())
//! }
//! ```
//! 
//! In most cases, you probably will need more than one transcation. The way that we have to do this now is to use an Enum as a main transaction that will be saved into the redolog. All transactions executed will then be converted into it.
//!
//! For more examples, take a look at the project tests


pub mod error;
mod redolog;
pub mod serializer;

use error::PrevaylerResult;
use redolog::ReDoLog;
use serializer::Serializer;

use async_std::path::Path;

/// The main Prevayler struct. This wrapper your data and save each executed transaction to the redolog.
/// Avoid creating it directly. Use [`PrevaylerBuilder`] instead.
pub struct Prevayler<D, T, S> {
    data: T,
    redo_log: ReDoLog<D, S>,
}

/// Builder of the [`Prevayler`] struct
pub struct PrevaylerBuilder<T, D, S, P> {
    path: Option<P>,
    serializer: Option<S>,
    data: Option<T>,
    _redolog: Option<ReDoLog<D, S>>,
    max_log_size: u64,
}

impl<T, D, S, P> PrevaylerBuilder<T, D, S, P> {
    /// Creates the builder
    pub fn new() -> Self {
        PrevaylerBuilder {
            path: None,
            serializer: None,
            data: None,
            _redolog: None,
            max_log_size: 64000,
        }
    }

    /// Set the path which will be used to store redologs and snapshots. The folder must exists.
    pub fn path(mut self, path: P) -> Self
    where
        P: AsRef<Path>,
    {
        self.path = Some(path);
        self
    }

    /// Set which serializer will be used. For more info, see [`Serializer`]
    pub fn serializer(mut self, serializer: S) -> Self
    where
        S: Serializer<D>,
    {
        self.serializer = Some(serializer);
        self
    }

    /// Set the max log size that will be stored in a single redolog file
    pub fn max_log_size(mut self, max_log_size: u64) -> Self {
        self.max_log_size = max_log_size;
        self
    }

    /// Set the data that will be prevailed
    pub fn data(mut self, data: T) -> Self {
        self.data = Some(data);
        self
    }

    /// Builds the Prevayler without snapshots. Notice that one of the Prevayler generic parameters cannot be infered by the compiler. This parameter is the type that will be serilized in the redolog.
    /// Also, if it is called without setting the path, serializer or data, this will panic.
    /// ```rust
    /// let mut prevayler: Prevayler<Increment, _, _> = PrevaylerBuilder::new()
    ///     .path(".")
    ///     .serializer(JsonSerializer::new())
    ///     .data(0 as u8)
    ///     .build().await?;
    /// ```
    pub async fn build(mut self) -> PrevaylerResult<Prevayler<D, T, S>>
    where
        D: Transaction<T>,
        S: Serializer<D>,
        P: AsRef<Path>,
    {
        Prevayler::new(
            self.path.expect("You need to define a path"),
            self.max_log_size,
            self.serializer.expect("You need to define a serializer"),
            self.data
                .take()
                .expect("You need to define a data to be prevailed"),
        )
        .await
    }

    /// Similar to the [`build`](PrevaylerBuilder::build), but this will try to process any saved snapshot. Note that this method has an extra trait bound. The Serializer must know who to serialize and deserialize the prevailed data type. 
    pub async fn build_with_snapshots(mut self) -> PrevaylerResult<Prevayler<D, T, S>>
    where
        D: Transaction<T>,
        S: Serializer<D> + Serializer<T>,
        P: AsRef<Path>,
    {
        Prevayler::new_with_snapshot(
            self.path.expect("You need to define a path"),
            self.max_log_size,
            self.serializer.expect("You need to define a serializer"),
            self.data
                .take()
                .expect("You need to define a data to be prevailed"),
        )
        .await
    }
}

impl<D, T, S> Prevayler<D, T, S> {
    async fn new<P>(path: P, max_log_size: u64, serializer: S, mut data: T) -> PrevaylerResult<Self>
    where
        D: Transaction<T>,
        S: Serializer<D>,
        P: AsRef<Path>,
    {
        let redo_log = ReDoLog::new(path, max_log_size, serializer, &mut data).await?;
        Ok(Prevayler {
            data,
            redo_log: redo_log,
        })
    }

    async fn new_with_snapshot<P>(
        path: P,
        max_log_size: u64,
        serializer: S,
        mut data: T,
    ) -> PrevaylerResult<Self>
    where
        D: Transaction<T>,
        S: Serializer<D> + Serializer<T>,
        P: AsRef<Path>,
    {
        let redo_log =
            ReDoLog::new_with_snapshot(path, max_log_size, serializer, &mut data).await?;
        Ok(Prevayler {
            data,
            redo_log: redo_log,
        })
    }

    /// Execute the given [transaction](Transaction) and write it to the redolog.
    /// You need to have a mutable reference to the prevayler. If you are in a multithread program, you can wrapp the prevayler behind a Mutex, RwLock or anyother concurrency control system.
    /// 
    /// This method returns a [`PrevaylerResult`]. If it the error is returned a [`PrevaylerError::SerializationError`](crate::error::PrevaylerError::SerializationError), then you can guarantee that the prevailed state did not change. But, if it returns the error a [`PrevaylerError::IOError`](crate::error::PrevaylerError::IOError), than the data did change but the redolog is in a inconsistent state. A solution would be to force a program restart.
    pub async fn execute_transaction<TR>(&mut self, transaction: TR) -> PrevaylerResult<()>
    where
        TR: Into<D>,
        D: Transaction<T>,
        S: Serializer<D>,
    {
        let transaction: D = transaction.into();
        let serialized = self.redo_log.serialize(&transaction)?;
        transaction.execute(&mut self.data);
        self.redo_log.write_to_log(serialized).await?;
        Ok(())
    }


    /// Like [execute_transaction](Prevayler::execute_transaction), it executes the give transaction and write it to the redolog. But, if a transaction panics for some reason, this gurantees that the prevailed state will not be changed.
    /// This gurantee comes wiht a cost. The entire prevailed state is cloned everytime that a transaction is executed.
    /// 
    /// Think twice before using this method. Your transactions should probably not be able to panic. Try to do any code that can fail before the transaction and only do the state change inside it.
    pub async fn execute_transaction_panic_safe<TR>(
        &mut self,
        transaction: TR,
    ) -> PrevaylerResult<()>
    where
        TR: Into<D>,
        D: Transaction<T>,
        S: Serializer<D>,
        T: Clone,
    {
        let transaction: D = transaction.into();
        let serialized = self.redo_log.serialize(&transaction)?;
        let mut data = self.data.clone();
        transaction.execute(&mut data);
        self.data = data;
        self.redo_log.write_to_log(serialized).await?;
        Ok(())
    }

    /// Does a snapshot of the prevailed state. This requires that the Serializer know how to serialize the prevailed state.
    pub async fn snapshot(&mut self) -> PrevaylerResult<()>
    where
        S: Serializer<T>,
    {
        self.redo_log.snapshot(&self.data).await?;
        Ok(())
    }

    /// Returns a reference to the prevailed state.
    pub fn query(&self) -> &T {
        &self.data
    }
}


/// The trait that defines the transaction behaivour.
/// You must implement it to all your transactions.
/// 
/// Notice that the execute method does not return a Result. You should handle your errors and gurantee that the prevailed state is in a consistent state.
/// 
/// Transactions should be deterministic. Do all non-deterministic code outside the transaction and let your transaction just update the values.
pub trait Transaction<T> {
    fn execute(self, data: &mut T);
}

#[cfg(test)]
mod tests {
    use super::{
        error::PrevaylerResult, serializer::JsonSerializer, Prevayler, PrevaylerBuilder,
        Transaction,
    };
    use async_std::sync::Mutex;
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;
    use std::thread;
    use temp_testdir::TempDir;

    #[derive(Serialize, Deserialize)]
    struct ChangeFirstElement {
        value: u8,
    }

    #[derive(Serialize, Deserialize)]
    struct ChangeSecondElement {
        value: u8,
    }

    #[derive(Serialize, Deserialize)]
    struct AddToFirstElement {
        value: u8,
    }

    #[derive(Serialize, Deserialize)]
    struct AddToSecondElementFailling {
        value: u8,
    }

    impl Transaction<(u8, u8)> for ChangeFirstElement {
        fn execute(self, data: &mut (u8, u8)) {
            data.0 = self.value;
        }
    }

    impl Transaction<(u8, u8)> for ChangeSecondElement {
        fn execute(self, data: &mut (u8, u8)) {
            data.1 = self.value;
        }
    }

    impl Transaction<(u8, u8)> for AddToFirstElement {
        fn execute(self, data: &mut (u8, u8)) {
            data.0 += self.value;
        }
    }

    impl Transaction<(u8, u8)> for AddToSecondElementFailling {
        fn execute(self, data: &mut (u8, u8)) {
            data.0 += self.value;
            panic!("Fail");
        }
    }

    #[derive(Serialize, Deserialize)]
    enum Transactions {
        ChangeFirstElement(ChangeFirstElement),
        ChangeSecondElement(ChangeSecondElement),
        AddToFirstElement(AddToFirstElement),
        AddToSecondElementFailling(AddToSecondElementFailling),
    }

    impl Transaction<(u8, u8)> for Transactions {
        fn execute(self, data: &mut (u8, u8)) {
            match self {
                Transactions::ChangeFirstElement(e) => {
                    e.execute(data);
                }
                Transactions::ChangeSecondElement(e) => {
                    e.execute(data);
                }
                Transactions::AddToFirstElement(e) => {
                    e.execute(data);
                }
                Transactions::AddToSecondElementFailling(e) => {
                    e.execute(data);
                }
            };
        }
    }

    impl Into<Transactions> for ChangeFirstElement {
        fn into(self) -> Transactions {
            Transactions::ChangeFirstElement(self)
        }
    }

    impl Into<Transactions> for ChangeSecondElement {
        fn into(self) -> Transactions {
            Transactions::ChangeSecondElement(self)
        }
    }

    impl Into<Transactions> for AddToFirstElement {
        fn into(self) -> Transactions {
            Transactions::AddToFirstElement(self)
        }
    }

    impl Into<Transactions> for AddToSecondElementFailling {
        fn into(self) -> Transactions {
            Transactions::AddToSecondElementFailling(self)
        }
    }

    #[async_std::test]
    async fn test_transaction() -> PrevaylerResult<()> {
        let temp = TempDir::default();
        let data = (3, 4);
        let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
            .path(&temp.as_os_str())
            .max_log_size(10)
            .serializer(JsonSerializer::new())
            .data(data)
            .build()
            .await?;
        prevayler
            .execute_transaction(ChangeFirstElement { value: 7 })
            .await?;
        prevayler
            .execute_transaction(ChangeSecondElement { value: 32 })
            .await?;
        assert_eq!(&(7, 32), prevayler.query());
        Ok(())
    }

    #[async_std::test]
    async fn test_multi_threading() -> PrevaylerResult<()> {
        let temp = TempDir::default();
        let data = (3, 4);
        let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
            .path(&temp.as_os_str())
            .max_log_size(10)
            .serializer(JsonSerializer::new())
            .data(data)
            .build()
            .await?;
        let prevayler = Arc::new(Mutex::new(prevayler));

        let prevayler_clone = prevayler.clone();
        let handle_1 = thread::spawn(move || {
            async_std::task::block_on(async {
                let mut guard = prevayler_clone.lock().await;
                guard
                    .execute_transaction(ChangeFirstElement { value: 7 })
                    .await
                    .expect("Error executing transaction")
            });
        });
        let prevayler_clone = prevayler.clone();
        let handle_2 = thread::spawn(move || {
            async_std::task::block_on(async {
                let mut guard = prevayler_clone.lock().await;
                guard
                    .execute_transaction(ChangeSecondElement { value: 32 })
                    .await
                    .expect("Error executing transaction")
            });
        });
        handle_1.join().unwrap();
        handle_2.join().unwrap();

        let guard = prevayler.lock().await;
        let query = guard.query();
        assert_eq!(7, query.0);
        assert_eq!(32, query.1);
        Ok(())
    }

    #[async_std::test]
    async fn test_panic_in_execute_transaction_panic_safe() -> PrevaylerResult<()> {
        let temp = TempDir::default();
        let data = (3, 4);
        let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
            .path(&temp.as_os_str())
            .max_log_size(10)
            .serializer(JsonSerializer::new())
            .data(data)
            .build()
            .await?;
        let prevayler = Arc::new(Mutex::new(prevayler));

        let prevayler_clone = prevayler.clone();
        let handle_1 = thread::spawn(move || {
            async_std::task::block_on(async {
                let mut guard = prevayler_clone.lock().await;
                guard
                    .execute_transaction(ChangeFirstElement { value: 7 })
                    .await
                    .expect("Error executing transaction")
            });
        });
        let prevayler_clone = prevayler.clone();
        let handle_2 = thread::spawn(move || {
            async_std::task::block_on(async {
                let mut guard = prevayler_clone.lock().await;
                guard
                    .execute_transaction_panic_safe(AddToSecondElementFailling { value: 32 })
                    .await
                    .expect("Error executing transaction")
            });
        });
        handle_1.join().unwrap();
        assert_eq!(true, handle_2.join().is_err());

        let guard = prevayler.lock().await;
        let query = guard.query();
        assert_eq!(7, query.0);
        assert_eq!(4, query.1);
        Ok(())
    }

    #[async_std::test]
    async fn test_should_save_state() -> PrevaylerResult<()> {
        let temp = TempDir::default();
        {
            let data = (3, 4);
            let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build()
                .await?;
            prevayler
                .execute_transaction(ChangeFirstElement { value: 7 })
                .await?;
        }
        {
            let data = (3, 4);
            let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build()
                .await?;
            prevayler
                .execute_transaction(ChangeSecondElement { value: 32 })
                .await?;
        }
        {
            let data = (3, 4);
            let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build()
                .await?;
            assert_eq!(&(7, 32), prevayler.query());
        }
        Ok(())
    }

    #[async_std::test]
    async fn test_redo_log_with_snapshot() -> PrevaylerResult<()> {
        let temp = TempDir::default();
        {
            let data = (3, 4);
            let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build_with_snapshots()
                .await?;
            prevayler
                .execute_transaction(AddToFirstElement { value: 7 })
                .await?;
            prevayler.snapshot().await?;
        }
        {
            let data = (3, 4);
            let mut prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build_with_snapshots()
                .await?;
            prevayler
                .execute_transaction(AddToFirstElement { value: 1 })
                .await?;
            assert_eq!(&(11, 4), prevayler.query());
        }
        {
            let data = (0, 0);
            let prevayler: Prevayler<Transactions, _, _> = PrevaylerBuilder::new()
                .path(&temp.as_os_str())
                .max_log_size(10)
                .serializer(JsonSerializer::new())
                .data(data)
                .build_with_snapshots()
                .await?;
            assert_eq!(&(11, 4), prevayler.query());
        }
        Ok(())
    }
}
