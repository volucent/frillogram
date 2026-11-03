//! Slack-style threads built from Telegram's ordinary reply chains.
//!
//! Nothing here is Frillogram-specific on the wire: a thread is a message plus
//! every loaded message that replies to it, directly or through other replies.
//! Replying in a thread sends a normal reply to the thread's root, so people on
//! other Telegram apps see a regular reply.

use std::collections::HashMap;

use crate::Msg;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// Index of the thread's first message in the slice passed to [`group`].
    pub root: usize,
    /// Indices of the replies, oldest first.
    pub replies: Vec<usize>,
    /// The root itself replies to a message that isn't loaded.
    pub root_is_reply: bool,
}

/// Groups `messages` (oldest first) into threads, ordered by root.
///
/// A reply whose parent isn't in `messages` starts its own thread, since there
/// is nothing loaded to attach it to. Parents are found by id, wherever they
/// sit: a pending reply's temporary id can be smaller than its root's once the
/// root is confirmed and gets its server id.
pub fn group(messages: &[Msg]) -> Vec<Thread> {
    let index_of: HashMap<i64, usize> = messages.iter().enumerate().map(|(i, m)| (m.id, i)).collect();
    let roots: Vec<usize> = (0..messages.len()).map(|i| root_of(i, messages, &index_of)).collect();

    // Threads first, so a reply can join a root that comes after it.
    let mut threads: Vec<Thread> = Vec::new();
    let mut thread_of_root: HashMap<usize, usize> = HashMap::new();
    for (i, message) in messages.iter().enumerate().filter(|&(i, _)| roots[i] == i) {
        thread_of_root.insert(i, threads.len());
        threads.push(Thread { root: i, replies: Vec::new(), root_is_reply: message.reply_to.is_some() });
    }
    for (i, &root) in roots.iter().enumerate().filter(|&(i, &root)| root != i) {
        threads[thread_of_root[&root]].replies.push(i);
    }
    threads
}

fn root_of(i: usize, messages: &[Msg], index_of: &HashMap<i64, usize>) -> usize {
    // A reply chain without cycles reaches its root within `messages.len()`
    // steps. Only malformed data loops longer: a message on a cycle, or whose
    // chain leads into one, becomes its own root.
    let mut root = i;
    for _ in 0..messages.len() {
        match messages[root].reply_to.and_then(|id| index_of.get(&id)) {
            Some(&parent) => root = parent,
            None => return root,
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SendState, SenderId};

    fn msg(id: i64, reply_to: Option<i64>) -> Msg {
        Msg {
            id,
            chat_id: 1,
            sender: SenderId::User(1),
            date: id as i32,
            text: format!("m{id}"),
            reply_to,
            outgoing: false,
            send_state: SendState::Sent,
        }
    }

    #[test]
    fn plain_messages_are_their_own_threads() {
        let threads = group(&[msg(1, None), msg(2, None)]);
        assert_eq!(threads.len(), 2);
        assert!(threads.iter().all(|t| t.replies.is_empty()));
    }

    #[test]
    fn nested_replies_join_the_root_thread() {
        // 3 replies to 1, 4 replies to 3: both belong to 1's thread.
        let messages = [msg(1, None), msg(2, None), msg(3, Some(1)), msg(4, Some(3))];
        let threads = group(&messages);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0], Thread { root: 0, replies: vec![2, 3], root_is_reply: false });
        assert_eq!(threads[1], Thread { root: 1, replies: vec![], root_is_reply: false });
    }

    #[test]
    fn reply_to_unloaded_message_starts_a_thread() {
        let threads = group(&[msg(5, Some(2)), msg(6, Some(5))]);
        assert_eq!(threads, vec![Thread { root: 0, replies: vec![1], root_is_reply: true }]);
    }

    #[test]
    fn pending_reply_stays_with_a_root_confirmed_after_it() {
        // The root was confirmed as 9 while its reply still has temporary id 7,
        // so sorted by id the reply comes first.
        let reply = Msg { send_state: SendState::Pending, ..msg(7, Some(9)) };
        let threads = group(&[msg(1, None), reply, msg(9, None)]);
        assert_eq!(
            threads,
            vec![
                Thread { root: 0, replies: vec![], root_is_reply: false },
                Thread { root: 2, replies: vec![1], root_is_reply: false },
            ]
        );
    }

    #[test]
    fn reply_cycles_do_not_panic() {
        // Malformed data: 1 and 2 reply to each other.
        let threads = group(&[msg(1, Some(2)), msg(2, Some(1)), msg(3, None)]);
        assert_eq!(threads.len(), 3);
        assert!(threads.iter().all(|t| t.replies.is_empty()));
    }
}
