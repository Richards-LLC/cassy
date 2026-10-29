//! Recording reader for CAS Factory terminal playback.
//!
//! The [`RecordingReader`] provides fast seeking and event iteration
//! for recorded terminal sessions.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use memmap2::Mmap;
use tracing::debug;

use crate::format::{FormatError, KeyframeEntry, KeyframeIndex, RecordingEvent, RecordingHeader};

/// Reader for recorded terminal sessions.
///
/// Provides fast seeking via keyframe index and sequential event reading.
/// The recording is decompressed on open for efficient random access.
///
/// # Example
///
/// ```ignore
/// let reader = RecordingReader::open("recording.rec")?;
///
/// // Get recording metadata
/// println!("Duration: {}ms", reader.duration_ms());
/// println!("Events: {}", reader.total_events());
///
/// // Seek to a specific timestamp
/// let position = reader.seek_to(45_000)?; // 45 seconds
///
/// // Read events from that position
/// for event in reader.read_events_from(position)? {
///     match event? {
///         RecordingEvent::Output { timestamp_ms, data } => {
///             // Process output...
///         }
///         _ => {}
///     }
/// }
/// ```
pub struct RecordingReader {
    /// Decompressed recording data
    data: RecordingData,
    /// Parsed recording header
    header: RecordingHeader,
    /// Keyframe index for fast seeking
    index: KeyframeIndex,
    /// Offset where events start (after header)
    events_start_offset: usize,
    /// Offset where keyframe index starts
    index_offset: usize,
}

/// Storage for decompressed recording data.
enum RecordingData {
    /// In-memory decompressed data (for smaller recordings)
    Memory(Vec<u8>),
    /// Memory-mapped decompressed temp file (for large recordings)
    Mmap(Mmap),
}

impl RecordingData {
    fn as_slice(&self) -> &[u8] {
        match self {
            RecordingData::Memory(v) => v,
            RecordingData::Mmap(m) => m,
        }
    }
}

/// Position within the recording for iteration.
#[derive(Debug, Clone, Copy)]
pub struct ReadPosition {
    /// Byte offset within decompressed data
    offset: usize,
    /// Current timestamp in milliseconds
    pub timestamp_ms: u64,
}

impl RecordingReader {
    /// Open a recording file for reading.
    ///
    /// Decompresses the file and parses the header and keyframe index.
    /// Returns an error if the file is corrupted or invalid.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, FormatError> {
        let path = path.as_ref();
        debug!("Opening recording: {:?}", path);

        // Read and decompress file
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let reader = BufReader::new(file);
        let mut decoder = zstd::stream::Decoder::new(reader)?;

        // Decompress to memory
        // For very large files (>100MB), we could decompress to a temp file and mmap
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;

        debug!(
            "Decompressed {} bytes -> {} bytes",
            file_size,
            decompressed.len()
        );

        Self::from_decompressed(decompressed)
    }

    /// Create a reader from already decompressed data.
    ///
    /// Useful for testing or when data is already in memory.
    pub fn from_decompressed(data: Vec<u8>) -> Result<Self, FormatError> {
        if data.len() < 12 {
            // Minimum: 4 (header_len) + 0 (header) + 8 (index_offset)
            return Err(FormatError::UnexpectedEof);
        }

        // Read index offset from last 8 bytes
        let index_offset_pos = data.len() - 8;
        let index_offset = u64::from_le_bytes(
            data[index_offset_pos..index_offset_pos + 8]
                .try_into()
                .map_err(|_| FormatError::CorruptedIndex)?,
        ) as usize;

        // Validate index offset
        if index_offset >= index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        // Read header
        let header_len = u32::from_le_bytes(
            data[0..4]
                .try_into()
                .map_err(|_| FormatError::UnexpectedEof)?,
        ) as usize;

        if 4 + header_len > data.len() {
            return Err(FormatError::UnexpectedEof);
        }

        let header: RecordingHeader = bincode::deserialize(&data[4..4 + header_len])?;
        header.validate()?;

        // Read keyframe index
        let index_len_pos = index_offset;
        if index_len_pos + 4 > index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        let index_len = u32::from_le_bytes(
            data[index_len_pos..index_len_pos + 4]
                .try_into()
                .map_err(|_| FormatError::CorruptedIndex)?,
        ) as usize;

        let index_data_start = index_len_pos + 4;
        if index_data_start + index_len > index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        let index: KeyframeIndex =
            bincode::deserialize(&data[index_data_start..index_data_start + index_len])?;

        let events_start_offset = 4 + header_len;

        debug!(
            "Loaded recording: {} events, {} keyframes, {}ms duration",
            index.total_events,
            index.entries.len(),
            index.total_duration_ms
        );

        Ok(Self {
            data: RecordingData::Memory(data),
            header,
            index,
            events_start_offset,
            index_offset,
        })
    }

    /// Open a large recording file using memory-mapped I/O.
    ///
    /// Decompresses to a temporary file and memory-maps it for efficient access.
    /// Recommended for recordings larger than 100MB decompressed.
    pub fn open_mmap<P: AsRef<Path>>(path: P) -> Result<Self, FormatError> {
        let path = path.as_ref();
        debug!("Opening recording with mmap: {:?}", path);

        // Read and decompress file
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut decoder = zstd::stream::Decoder::new(reader)?;

        // Create temp file for decompressed data
        let mut temp_file = tempfile::tempfile()?;
        std::io::copy(&mut decoder, &mut temp_file)?;

        // Memory-map the temp file
        let mmap = unsafe { Mmap::map(&temp_file)? };
        let data_len = mmap.len();

        if data_len < 12 {
            return Err(FormatError::UnexpectedEof);
        }

        // Parse using the mmap data
        let data_slice: &[u8] = &mmap;

        // Read index offset from last 8 bytes
        let index_offset_pos = data_len - 8;
        let index_offset = u64::from_le_bytes(
            data_slice[index_offset_pos..index_offset_pos + 8]
                .try_into()
                .map_err(|_| FormatError::CorruptedIndex)?,
        ) as usize;

        if index_offset >= index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        // Read header
        let header_len = u32::from_le_bytes(
            data_slice[0..4]
                .try_into()
                .map_err(|_| FormatError::UnexpectedEof)?,
        ) as usize;

        if 4 + header_len > data_len {
            return Err(FormatError::UnexpectedEof);
        }

        let header: RecordingHeader = bincode::deserialize(&data_slice[4..4 + header_len])?;
        header.validate()?;

        // Read keyframe index
        let index_len_pos = index_offset;
        if index_len_pos + 4 > index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        let index_len = u32::from_le_bytes(
            data_slice[index_len_pos..index_len_pos + 4]
                .try_into()
                .map_err(|_| FormatError::CorruptedIndex)?,
        ) as usize;

        let index_data_start = index_len_pos + 4;
        if index_data_start + index_len > index_offset_pos {
            return Err(FormatError::CorruptedIndex);
        }

        let index: KeyframeIndex =
            bincode::deserialize(&data_slice[index_data_start..index_data_start + index_len])?;

        let events_start_offset = 4 + header_len;

        debug!(
            "Loaded recording (mmap): {} events, {} keyframes, {}ms duration",
            index.total_events,
            index.entries.len(),
            index.total_duration_ms
        );

        Ok(Self {
            data: RecordingData::Mmap(mmap),
            header,
            index,
            events_start_offset,
            index_offset,
        })
    }

    /// Get the recording header.
    pub fn header(&self) -> &RecordingHeader {
        &self.header
    }

    /// Get the keyframe index.
    pub fn keyframe_index(&self) -> &KeyframeIndex {
        &self.index
    }

    /// Get the total duration in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.index.total_duration_ms
    }

    /// Get the total number of events.
    pub fn total_events(&self) -> u64 {
        self.index.total_events
    }

    /// Get the number of keyframes.
    pub fn keyframe_count(&self) -> usize {
        self.index.entries.len()
    }

    /// Seek to the nearest keyframe at or before the given timestamp.
    ///
    /// Returns a [`ReadPosition`] that can be used with [`read_events_from`].
    /// Uses binary search on the keyframe index for O(log n) seeking.
    ///
    /// # Performance
    ///
    /// Seeking is O(log k) where k is the number of keyframes.
    /// For a 2-hour recording with 30-second keyframes (240 keyframes),
    /// this is approximately 8 comparisons.
    pub fn seek_to(&self, timestamp_ms: u64) -> Result<ReadPosition, FormatError> {
        // Clamp to recording duration
        let timestamp_ms = timestamp_ms.min(self.index.total_duration_ms);

        // Find the best keyframe using binary search
        if let Some(keyframe) = self.index.find_keyframe(timestamp_ms) {
            Ok(ReadPosition {
                offset: keyframe.event_offset as usize,
                timestamp_ms: keyframe.timestamp_ms,
            })
        } else {
            // No keyframes, start from the beginning
            Ok(ReadPosition {
                offset: self.events_start_offset,
                timestamp_ms: 0,
            })
        }
    }

    /// Seek to the beginning of the recording.
    pub fn seek_to_start(&self) -> ReadPosition {
        ReadPosition {
            offset: self.events_start_offset,
            timestamp_ms: 0,
        }
    }

    /// Read the snapshot data for a keyframe.
    ///
    /// Returns the raw snapshot bytes that can be deserialized into terminal state.
    pub fn read_snapshot(&self, keyframe: &KeyframeEntry) -> Result<Vec<u8>, FormatError> {
        let data = self.data.as_slice();
        let offset = keyframe.snapshot_offset as usize;

        if offset + 4 > data.len() {
            return Err(FormatError::UnexpectedEof);
        }

        let snapshot_len = u32::from_le_bytes(
            data[offset..offset + 4]
                .try_into()
                .map_err(|_| FormatError::UnexpectedEof)?,
        ) as usize;

        let data_start = offset + 4;
        if data_start + snapshot_len > data.len() {
            return Err(FormatError::UnexpectedEof);
        }

        Ok(data[data_start..data_start + snapshot_len].to_vec())
    }

    /// Create an iterator over events starting from the given position.
    ///
    /// Events are yielded in chronological order until the end of the recording
    /// or the keyframe index is reached.
    pub fn read_events_from(&self, position: ReadPosition) -> EventIterator<'_> {
        // Build skip regions from keyframe snapshots
        // Each snapshot is stored as: u32 length + snapshot bytes
        let skip_regions: Vec<(usize, usize)> = self
            .index
            .entries
            .iter()
            .map(|kf| {
                let start = kf.snapshot_offset as usize;
                // Read snapshot length to calculate end
                let len = if start + 4 <= self.data.as_slice().len() {
                    u32::from_le_bytes(
                        self.data.as_slice()[start..start + 4]
                            .try_into()
                            .unwrap_or([0; 4]),
                    ) as usize
                } else {
                    0
                };
                (start, start + 4 + len) // (start, end) of snapshot region
            })
            .collect();

        EventIterator {
            data: self.data.as_slice(),
            offset: position.offset,
            end_offset: self.index_offset,
            skip_regions,
        }
    }

    /// Read all events in the recording.
    pub fn read_all_events(&self) -> EventIterator<'_> {
        self.read_events_from(self.seek_to_start())
    }

    /// Read events within a timestamp range.
    ///
    /// Seeks to the keyframe before `start_ms` and iterates until `end_ms`.
    pub fn read_events_in_range(
        &self,
        start_ms: u64,
        end_ms: u64,
    ) -> Result<impl Iterator<Item = Result<RecordingEvent, FormatError>> + '_, FormatError> {
        let position = self.seek_to(start_ms)?;
        Ok(self
            .read_events_from(position)
            .take_while(move |result| match result {
                Ok(event) => event.timestamp_ms() <= end_ms,
                Err(_) => true, // Propagate errors
            }))
    }
}

/// Iterator over recording events.
pub struct EventIterator<'a> {
    data: &'a [u8],
    offset: usize,
    end_offset: usize,
    /// Regions to skip (snapshot data): Vec of (start, end) byte offsets
    skip_regions: Vec<(usize, usize)>,
}

impl<'a> EventIterator<'a> {
    /// Check if current offset is in a skip region and return the end of that region
    fn in_skip_region(&self) -> Option<usize> {
        for &(start, end) in &self.skip_regions {
            if self.offset >= start && self.offset < end {
                return Some(end);
            }
        }
        None
    }
}

impl<'a> Iterator for EventIterator<'a> {
    type Item = Result<RecordingEvent, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        // Skip over snapshot regions
        while let Some(skip_end) = self.in_skip_region() {
            self.offset = skip_end;
        }

        // Check if we've reached the index
        if self.offset >= self.end_offset {
            return None;
        }

        // Check for enough bytes for length prefix
        if self.offset + 4 > self.data.len() {
            return Some(Err(FormatError::UnexpectedEof));
        }

        // Read event length
        let event_len = match self.data[self.offset..self.offset + 4].try_into() {
            Ok(bytes) => u32::from_le_bytes(bytes) as usize,
            Err(_) => return Some(Err(FormatError::UnexpectedEof)),
        };

        let event_start = self.offset + 4;
        let event_end = event_start + event_len;

        // Check bounds
        if event_end > self.data.len() || event_end > self.end_offset + 4 {
            return Some(Err(FormatError::UnexpectedEof));
        }

        // Deserialize event
        let event: RecordingEvent = match bincode::deserialize(&self.data[event_start..event_end]) {
            Ok(e) => e,
            Err(e) => return Some(Err(FormatError::Bincode(e))),
        };

        // Advance offset
        self.offset = event_end;

        Some(Ok(event))
    }
}

#[cfg(test)]
mod tests {
    use crate::reader::*;
    use crate::writer::{RecordingWriter, WriterConfig};
    use tempfile::TempDir;

    async fn create_test_recording(dir: &TempDir) -> std::path::PathBuf {
        let config = WriterConfig {
            recordings_dir: dir.path().to_path_buf(),
            keyframe_interval_ms: 100,
            compression_level: 1,
            buffer_size: 16,
        };

        let mut writer =
            RecordingWriter::new(80, 24, "test-agent", "test-session", "worker", config)
                .await
                .unwrap();

        // Write some events
        writer.write_output(b"Hello, ").await.unwrap();
        writer.write_output(b"World!\n").await.unwrap();
        writer.write_resize(120, 40).await.unwrap();
        writer
            .write_keyframe(b"snapshot-data-here".to_vec())
            .await
            .unwrap();
        writer.write_output(b"After keyframe").await.unwrap();

        let file_path = writer.file_path().clone();
        writer.close().await.unwrap();

        file_path
    }

    #[tokio::test]
    async fn test_reader_opens_file() {
        let dir = TempDir::new().unwrap();
        let path = create_test_recording(&dir).await;

        let reader = RecordingReader::open(&path).unwrap();

        assert_eq!(reader.header().cols, 80);
        assert_eq!(reader.header().rows, 24);
        assert_eq!(reader.header().agent_name, "test-agent");
        // 5 events: 2 outputs + resize + keyframe + 1 output after keyframe
        assert_eq!(reader.total_events(), 5);
        assert_eq!(reader.keyframe_count(), 1);
    }

    #[tokio::test]
    async fn test_reader_mmap_opens_file() {
        let dir = TempDir::new().unwrap();
        let path = create_test_recording(&dir).await;

        let reader = RecordingReader::open_mmap(&path).unwrap();

        assert_eq!(reader.header().cols, 80);
        assert_eq!(reader.total_events(), 5);
    }

    #[tokio::test]
    async fn test_reader_iterates_events() {
        let dir = TempDir::new().unwrap();
        let path = create_test_recording(&dir).await;

        let reader = RecordingReader::open(&path).unwrap();
        let events: Vec<_> = reader.read_all_events().collect();

        // 5 events: Output, Output, Resize, Keyframe, Output
        assert_eq!(events.len(), 5);

        // Check first event
        match &events[0] {
            Ok(RecordingEvent::Output { data, .. }) => {
                assert_eq!(data, b"Hello, ");
            }
            _ => panic!("Expected Output event"),
        }

        // Check resize event (index 2)
        match &events[2] {
            Ok(RecordingEvent::Resize { cols, rows, .. }) => {
                assert_eq!(*cols, 120);
                assert_eq!(*rows, 40);
            }
            _ => panic!("Expected Resize event"),
        }

        // Check keyframe event (index 3)
        match &events[3] {
            Ok(RecordingEvent::Keyframe { .. }) => {}
            _ => panic!("Expected Keyframe event"),
        }

        // Check last output event (index 4)
        match &events[4] {
            Ok(RecordingEvent::Output { data, .. }) => {
                assert_eq!(data, b"After keyframe");
            }
            _ => panic!("Expected Output event after keyframe"),
        }
    }

    #[tokio::test]
    async fn test_reader_seek_to_keyframe() {
        let dir = TempDir::new().unwrap();
        let path = create_test_recording(&dir).await;

        let reader = RecordingReader::open(&path).unwrap();

        // Seek to a time after the keyframe should find the keyframe
        let position = reader.seek_to(50).unwrap();

        // Should be at or near the keyframe timestamp
        assert!(position.timestamp_ms <= 50 || reader.keyframe_count() == 0);
    }

    #[tokio::test]
    async fn test_reader_read_snapshot() {
        let dir = TempDir::new().unwrap();
        let path = create_test_recording(&dir).await;

        let reader = RecordingReader::open(&path).unwrap();

        if let Some(keyframe) = reader.keyframe_index().entries.first() {
            let snapshot = reader.read_snapshot(keyframe).unwrap();
            assert_eq!(snapshot, b"snapshot-data-here");
        }
    }

    #[tokio::test]
    async fn test_reader_handles_empty_recording() {
        let dir = TempDir::new().unwrap();
        let config = WriterConfig {
            recordings_dir: dir.path().to_path_buf(),
            keyframe_interval_ms: 30000,
            compression_level: 1,
            buffer_size: 16,
        };

        let writer = RecordingWriter::new(80, 24, "empty-agent", "empty-session", "worker", config)
            .await
            .unwrap();

        let file_path = writer.file_path().clone();
        writer.close().await.unwrap();

        let reader = RecordingReader::open(&file_path).unwrap();

        assert_eq!(reader.total_events(), 0);
        assert_eq!(reader.keyframe_count(), 0);
        assert_eq!(reader.duration_ms(), 0);

        let events: Vec<_> = reader.read_all_events().collect();
        assert!(events.is_empty());
    }

    #[test]
    fn test_reader_handles_truncated_file() {
        // Create truncated data (too short)
        let result = RecordingReader::from_decompressed(vec![0, 1, 2, 3]);
        assert!(result.is_err());
    }

    #[test]
    fn test_reader_handles_invalid_index_offset() {
        // Create data with invalid index offset (pointing past end)
        let mut data = vec![0u8; 100];
        // Set index_offset to something huge
        let invalid_offset: u64 = 999999;
        data[92..100].copy_from_slice(&invalid_offset.to_le_bytes());

        let result = RecordingReader::from_decompressed(data);
        assert!(matches!(result, Err(FormatError::CorruptedIndex)));
    }

    #[tokio::test]
    async fn test_repeated_seeks_return_the_latest_keyframe_before_each_target() {
        let dir = TempDir::new().unwrap();
        let config = WriterConfig {
            recordings_dir: dir.path().to_path_buf(),
            keyframe_interval_ms: 100,
            compression_level: 1,
            buffer_size: 256,
        };

        let mut writer =
            RecordingWriter::new(80, 24, "perf-agent", "perf-session", "worker", config)
                .await
                .unwrap();

        // Write many events with keyframes
        for i in 0..100 {
            writer
                .write_output(format!("Event {i}\n").as_bytes())
                .await
                .unwrap();
            if i % 10 == 0 {
                writer
                    .write_keyframe(format!("Snapshot {i}").into_bytes())
                    .await
                    .unwrap();
            }
        }

        let file_path = writer.file_path().clone();
        writer.close().await.unwrap();

        let reader = RecordingReader::open(&file_path).unwrap();

        // Exercise a broad set of targets repeatedly, checking the seek
        // contract against a simple linear reference instead of a host-speed
        // budget. This still catches wrong index ordering and boundary picks.
        let entries = &reader.keyframe_index().entries;
        let duration = reader.duration_ms();
        let max_comparisons =
            (entries.len() + 1).next_power_of_two().trailing_zeros() as usize;
        for n in 0..1_000_u64 {
            let target = n * duration / 999;
            let expected = entries
                .iter()
                .rev()
                .find(|entry| entry.timestamp_ms <= target)
                .or_else(|| entries.first())
                .map_or(0, |entry| entry.timestamp_ms);
            let position = reader.seek_to(target).unwrap();
            assert_eq!(position.timestamp_ms, expected, "target {target}ms");
            assert!(
                reader
                    .keyframe_index()
                    .find_keyframe_comparison_count(target)
                    <= max_comparisons,
                "seek index inspected more than {max_comparisons} entries for {target}ms"
            );
        }
    }
}
