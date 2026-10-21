//! An in-memory Telegram for tests: records what the bot sends and edits, serves queued updates.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::telegram::api::{Api, ApiError, Update};
use crate::telegram::render::{Button, Keyboard};

#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub chat: i64,
    pub message_id: i64,
    pub text: String,
    pub html: bool,
    pub keyboard: Keyboard,
    pub reply_to: Option<i64>,
}

pub struct Fake {
    /// Served by get_updates while their id is at least the offset, as Telegram does.
    pub updates: Vec<Update>,
    pub sent: Vec<Out>,
    pub edits: Vec<Out>,
    pub presses: Vec<(String, String)>,
    /// The next calls fail with these, in order.
    pub fail: VecDeque<ApiError>,
    /// Refuse HTML, as Telegram does for text it cannot parse.
    pub refuse_html: bool,
    /// The offset of every successful get_updates.
    pub offsets: Vec<i64>,
    /// Set the flag once this many get_updates have succeeded.
    pub stop: Option<(usize, Arc<AtomicBool>)>,
    next: i64,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            updates: Vec::new(),
            sent: Vec::new(),
            edits: Vec::new(),
            presses: Vec::new(),
            fail: VecDeque::new(),
            refuse_html: false,
            offsets: Vec::new(),
            stop: None,
            next: 100,
        }
    }
}

impl Fake {
    fn failing(&mut self) -> Result<(), ApiError> {
        self.fail.pop_front().map_or(Ok(()), Err)
    }

    fn html(&self, html: bool) -> Result<(), ApiError> {
        if html && self.refuse_html {
            Err(ApiError::Refused(
                "Bad Request: can't parse entities".into(),
            ))
        } else {
            Ok(())
        }
    }
}

impl Api for Fake {
    fn get_me(&mut self) -> Result<String, ApiError> {
        self.failing()?;
        Ok("fake_bot".into())
    }

    fn get_updates(&mut self, offset: i64, _timeout: u64) -> Result<Vec<Update>, ApiError> {
        self.failing()?;
        self.offsets.push(offset);
        if let Some((n, flag)) = &self.stop
            && self.offsets.len() >= *n
        {
            flag.store(true, Ordering::Relaxed);
        }
        Ok(self
            .updates
            .iter()
            .filter(|u| u.id >= offset)
            .cloned()
            .collect())
    }

    fn send(
        &mut self,
        chat: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError> {
        self.failing()?;
        self.html(html)?;
        let message_id = self.next;
        self.next += 1;
        self.sent.push(Out {
            chat,
            message_id,
            text: text.into(),
            html,
            keyboard: keyboard.to_vec(),
            reply_to,
        });
        Ok(message_id)
    }

    fn edit(
        &mut self,
        chat: i64,
        message_id: i64,
        text: &str,
        html: bool,
        keyboard: &[Vec<Button>],
    ) -> Result<(), ApiError> {
        self.failing()?;
        self.html(html)?;
        self.edits.push(Out {
            chat,
            message_id,
            text: text.into(),
            html,
            keyboard: keyboard.to_vec(),
            reply_to: None,
        });
        Ok(())
    }

    fn answer_press(&mut self, query_id: &str, text: &str) -> Result<(), ApiError> {
        self.failing()?;
        self.presses.push((query_id.into(), text.into()));
        Ok(())
    }
}

/// An idle source that says what the test tells it.
pub struct FixedIdle(pub std::rc::Rc<std::cell::Cell<Option<u64>>>);

impl crate::idle::Idle for FixedIdle {
    fn seconds(&mut self) -> Option<u64> {
        self.0.get()
    }
}
