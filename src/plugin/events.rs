//! Events between plugins, and between the engine and plugins.
//!
//! An event type is a name and a size; an event is that many bytes. Whoever knows the name
//! can send or read, in any language: this is how one plugin tells another that something
//! happened without either knowing the other exists. The engine publishes its own events the
//! same way (`mira.Contact`, from physics).
//!
//! Like the typed `Events<E>`, a channel keeps what was sent this frame and last frame, and
//! each reader has its own place, so every reader sees each event exactly once whichever
//! order the systems run in.
//!
//! A reader's place is kept here, under the reader's name, not in the reader. A plugin system
//! that is hot-reloaded is a new object with the same name, so it carries on exactly where
//! the old one stopped: nothing missed, nothing seen twice.

use std::{collections::HashMap, sync::Mutex};

struct Channel {
    name: String,
    size: usize,
    /// Events sent last frame and this frame, end to end.
    old: Vec<u8>,
    new: Vec<u8>,
    old_count: usize,
    new_count: usize,
    /// The number of the first event in `old`.
    old_start: usize,
}

/// Every event channel defined at runtime.
#[derive(Default)]
pub struct PluginEvents {
    channels: Vec<Channel>,
    names: HashMap<String, u32>,
    /// For each reader, the number of the next event it should see on each channel. Behind a
    /// lock because reading advances it, and readers only have shared access.
    readers: Mutex<Vec<HashMap<u32, usize>>>,
    reader_names: Mutex<HashMap<String, u32>>,
}

impl PluginEvents {
    /// Defines an event type, or finds the one already defined under `name`. Fails if it
    /// exists with a different size, since the two sides would misread each other.
    pub fn register(&mut self, name: &str, size: usize) -> Result<u32, String> {
        if let Some(&id) = self.names.get(name) {
            let existing = self.channels[id as usize].size;
            if existing != size {
                return Err(format!(
                    "event `{name}` is {existing} bytes, but was asked for as {size} bytes"
                ));
            }
            return Ok(id);
        }
        let id = self.channels.len() as u32;
        self.channels.push(Channel {
            name: name.to_owned(),
            size,
            old: Vec::new(),
            new: Vec::new(),
            old_count: 0,
            new_count: 0,
            old_start: 0,
        });
        self.names.insert(name.to_owned(), id);
        Ok(id)
    }

    pub fn id(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }

    pub fn name(&self, id: u32) -> Option<&str> {
        self.channels.get(id as usize).map(|c| c.name.as_str())
    }

    pub fn size(&self, id: u32) -> Option<usize> {
        self.channels.get(id as usize).map(|c| c.size)
    }

    pub fn len(&self) -> usize {
        self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }

    /// Sends an event. `bytes` must be exactly the event's size; anything else is dropped.
    pub fn send(&mut self, id: u32, bytes: &[u8]) {
        if let Some(channel) = self.channels.get_mut(id as usize) {
            if bytes.len() == channel.size {
                channel.new.extend_from_slice(bytes);
                channel.new_count += 1;
            }
        }
    }

    /// Sends a plain value as an event, for engine code publishing to plugins.
    pub fn send_value<T: bytemuck::NoUninit>(&mut self, id: u32, value: &T) {
        self.send(id, bytemuck::bytes_of(value));
    }

    /// The number the next event sent on this channel will have.
    pub fn end(&self, id: u32) -> usize {
        self.channels
            .get(id as usize)
            .map_or(0, |c| c.old_start + c.old_count + c.new_count)
    }

    /// The reader with this name, made if this is the first time it is asked for. A new
    /// reader starts at the oldest event still kept on every channel.
    pub fn reader(&self, name: &str) -> u32 {
        let mut names = self.reader_names.lock().unwrap();
        if let Some(&reader) = names.get(name) {
            return reader;
        }
        let mut readers = self.readers.lock().unwrap();
        readers.push(HashMap::new());
        let reader = readers.len() as u32 - 1;
        names.insert(name.to_owned(), reader);
        reader
    }

    /// The next event on a channel that this reader hasn't seen, if any.
    pub fn next(&self, reader: u32, id: u32) -> Option<&[u8]> {
        let mut readers = self.readers.lock().unwrap();
        let cursor = readers.get_mut(reader as usize)?.entry(id).or_insert(0);
        let (bytes, next) = self.read(id, *cursor)?;
        *cursor = next;
        Some(bytes)
    }

    /// The event numbered `cursor`, or the oldest one still kept if that one is gone, with
    /// the cursor to use next. `None` when the reader has seen everything.
    pub fn read(&self, id: u32, cursor: usize) -> Option<(&[u8], usize)> {
        let channel = self.channels.get(id as usize)?;
        let index = cursor.max(channel.old_start) - channel.old_start;
        let bytes = if index < channel.old_count {
            &channel.old[index * channel.size..][..channel.size]
        } else if index - channel.old_count < channel.new_count {
            &channel.new[(index - channel.old_count) * channel.size..][..channel.size]
        } else {
            return None;
        };
        Some((bytes, channel.old_start + index + 1))
    }

    /// Drops the events from the frame before last. Called once per frame.
    pub fn update(&mut self) {
        for channel in &mut self.channels {
            channel.old_start += channel.old_count;
            std::mem::swap(&mut channel.old, &mut channel.new);
            channel.old_count = channel.new_count;
            channel.new.clear();
            channel.new_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reader_sees_each_event_once() {
        let mut events = PluginEvents::default();
        let id = events.register("test.Number", 4).unwrap();
        assert_eq!(events.register("test.Number", 4), Ok(id));
        assert!(events.register("test.Number", 8).is_err());

        let read_all = |events: &PluginEvents, cursor: &mut usize| {
            let mut seen = Vec::new();
            while let Some((bytes, next)) = events.read(id, *cursor) {
                seen.push(u32::from_ne_bytes(bytes.try_into().unwrap()));
                *cursor = next;
            }
            seen
        };
        let (mut early, mut late) = (0, 0);

        events.send_value(id, &1u32);
        assert_eq!(read_all(&events, &mut early), [1]);
        events.send_value(id, &2u32);
        events.send(id, &[0; 3]); // the wrong size: dropped
        events.update();
        events.send_value(id, &3u32);
        assert_eq!(read_all(&events, &mut early), [2, 3]);
        assert_eq!(
            read_all(&events, &mut late),
            [1, 2, 3],
            "a reader that runs later misses nothing"
        );
        assert_eq!(read_all(&events, &mut late), [] as [u32; 0]);

        // Events last two frames; a reader that sleeps longer skips what it missed.
        let mut asleep = 0;
        events.update();
        events.send_value(id, &4u32);
        events.update();
        assert_eq!(read_all(&events, &mut asleep), [4]);
        assert_eq!(events.end(id), 4);

        // A named reader's place is kept for it, so whoever asks under that name next (a
        // reloaded system) carries on from there.
        let before_reload = events.reader("plugin::listen");
        assert_eq!(
            events.next(before_reload, id).map(<[u8]>::to_vec),
            Some(4u32.to_ne_bytes().to_vec())
        );
        events.send_value(id, &5u32);
        let after_reload = events.reader("plugin::listen");
        assert_eq!(
            events.next(after_reload, id).map(<[u8]>::to_vec),
            Some(5u32.to_ne_bytes().to_vec())
        );
        assert!(events.next(after_reload, id).is_none());
    }
}
