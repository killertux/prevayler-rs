use async_std::fs::{read_dir, File, OpenOptions};
use async_std::io::BufReader;
use async_std::path::{Path, PathBuf};
use async_std::prelude::*;
use std::marker::PhantomData;

use super::serializer::Serializer;
use crate::{error::PrevaylerResult, Transaction};

pub struct ReDoLog<D, S> {
    path: PathBuf,
    max_size: u64,
    current_size: u64,
    index: u64,
    file: File,
    serializer: S,
    _p: PhantomData<D>,
}

async fn files(path: PathBuf) -> std::io::Result<Vec<PathBuf>> {
    let mut entries = read_dir(path.clone()).await?;
    let mut files = Vec::new();
    while let Some(res) = entries.next().await {
        let entry = res?;
        if !entry
            .file_type()
            .await
            .expect("Error getting file type")
            .is_file()
        {
            continue;
        }
        let filename = entry.file_name().into_string().expect("Error parsing filename");
        if !(filename.starts_with("redolog") || filename.starts_with("snapshot")) {
            continue;
        }
        files.push(entry.path())
    }
    Ok(files)
}

impl<D, S> ReDoLog<D, S> {
    pub async fn new<T, P>(
        path: P,
        max_size: u64,
        serializer: S,
        data: &mut T,
    ) -> PrevaylerResult<Self>
    where
        D: Transaction<T>,
        S: Serializer<D>,
        P: AsRef<Path>,
    {
        let path = PathBuf::from(path.as_ref());
        let mut files = files(path.clone()).await?;
        files.sort();
        Self::new_from_files(path, files, 0, max_size, serializer, data).await
    }

    pub async fn new_with_snapshot<T, P>(
        path: P,
        max_size: u64,
        serializer: S,
        data: &mut T,
    ) -> PrevaylerResult<Self>
    where
        D: Transaction<T>,
        S: Serializer<D> + Serializer<T>,
        P: AsRef<Path>,
    {
        let path = PathBuf::from(path.as_ref());
        let files = files(path.clone()).await?;
        let (index, files) = Self::process_snapshots(files, &serializer, data).await?;
        ReDoLog::new_from_files(path, files, index, max_size, serializer, data).await
    }

    pub fn serialize(&self, data: &D) -> PrevaylerResult<Box<[u8]>>
    where
        S: Serializer<D>,
    {
        Ok(self.serializer.serialize(&data)?)
    }

    pub async fn write_to_log(&mut self, data: Box<[u8]>) -> std::io::Result<()>
    where
        S: Serializer<D>,
    {
        self.current_size += 1;
        self.create_new_log_if_necessary().await?;
        self.file.write_all(&data).await?;
        self.file.flush().await?;
        Ok(())
    }

    pub async fn snapshot<T>(&mut self, data: &T) -> PrevaylerResult<()>
    where
        S: Serializer<T>,
    {
        let mut path = self.path.clone();
        self.index += 1;
        path.push(format!("snapshot.log.{:0>10}", self.index));
        let mut file = File::create(path).await?;
        file.write_all(&self.serializer.serialize(&data)?).await?;
        file.flush().await?;
        self.create_new_log().await?;
        Ok(())
    }

    async fn process_snapshots<T>(
        mut files: Vec<PathBuf>,
        serializer: &S,
        data: &mut T,
    ) -> PrevaylerResult<(u64, Vec<PathBuf>)>
    where
        S: Serializer<T>,
    {
        if files.is_empty() {
            return Ok((0, files));
        }
        files.sort_by(|file_a, file_b| {
            get_index_from_file(file_a).cmp(&get_index_from_file(file_b))
        });
        files.reverse();
        let position = files.iter().position(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("snapshot")
        });
        let (index, mut files) = match position {
            Some(position) => {
                let (files_to_process_without_snapshot, rest_of_files_with_snapshot) =
                    files.split_at(position);
                let (snapshot, _) = rest_of_files_with_snapshot.split_first().unwrap();
                let index = get_index_from_file(snapshot) + 1;
                *data = serializer
                    .deserialize(BufReader::new(File::open(snapshot.clone()).await?))
                    .next()
                    .await
                    .unwrap()?;
                (index, files_to_process_without_snapshot.to_vec())
            }
            None => (0, files),
        };
        files.reverse();
        Ok((index, files))
    }

    async fn new_from_files<T>(
        path: PathBuf,
        files: Vec<PathBuf>,
        index: u64,
        max_size: u64,
        serializer: S,
        data: &mut T,
    ) -> PrevaylerResult<Self>
    where
        D: Transaction<T>,
        S: Serializer<D>,
    {
        if files.is_empty() {
            return Self::new_empty(path, index, max_size, serializer).await;
        }
        let (index, lines_count, last_file_name) =
            Self::process_files::<T>(&serializer, files, data).await?;
        Ok(ReDoLog {
            path: path,
            max_size,
            index: index,
            current_size: lines_count,
            serializer: serializer,
            file: OpenOptions::new().append(true).open(last_file_name).await?,
            _p: PhantomData {},
        })
    }

    async fn new_empty(
        path: PathBuf,
        index: u64,
        max_size: u64,
        serializer: S,
    ) -> PrevaylerResult<Self> {
        let file_path = Self::file_name_for_index(path.clone(), index);
        Ok(ReDoLog {
            path: path,
            max_size,
            current_size: 0,
            index,
            serializer: serializer,
            file: File::create(file_path).await?,
            _p: PhantomData {},
        })
    }

    async fn process_files<T>(
        serializer: &S,
        files: Vec<PathBuf>,
        data: &mut T,
    ) -> PrevaylerResult<(u64, u64, String)>
    where
        D: Transaction<T>,
        S: Serializer<D>,
    {
        let mut index = 0;
        let mut log_count = 0;
        let mut last_file_name = String::new();
        for file in files {
            log_count = 0;
            last_file_name = file.to_str().unwrap().to_owned();
            let mut transactions =
                serializer.deserialize(BufReader::new(File::open(file.clone()).await?));
            while let Some(transaction) = transactions.next().await {
                let transaction = transaction?;
                log_count += 1;
                transaction.execute(data);
            }
            index = get_index_from_file(&file);
        }
        Ok((index, log_count, last_file_name))
    }

    async fn create_new_log_if_necessary(&mut self) -> std::io::Result<()> {
        if self.current_size >= self.max_size {
            self.create_new_log().await?;
        }
        Ok(())
    }

    async fn create_new_log(&mut self) -> std::io::Result<()> {
        self.index += 1;
        self.current_size = 0;
        self.file = File::create(Self::file_name_for_index(self.path.clone(), self.index)).await?;
        Ok(())
    }

    fn file_name_for_index(mut path: PathBuf, index: u64) -> PathBuf {
        path.push(format!("redolog.log.{:0>10}", index));
        path
    }
}

fn get_index_from_file(path: &PathBuf) -> u64 {
    path.extension()
        .unwrap()
        .to_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
}
