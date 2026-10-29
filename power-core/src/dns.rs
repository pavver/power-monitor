//! Captive-portal DNS: every A query is answered with the device's own address,
//! so a phone joining the setup network opens the settings page by itself.

const HEADER_LEN: usize = 12;
const TYPE_A: u16 = 1;
const CLASS_IN: u16 = 1;
const TTL_SECONDS: u32 = 60;

/// The reply to `query`, or `None` when it is not a well-formed standard query.
/// Non-A questions get an empty answer so clients fall back to IPv4.
pub fn answer(query: &[u8], ip: [u8; 4]) -> Option<Vec<u8>> {
    let header = query.get(..HEADER_LEN)?;
    // QR = 0 (query) and opcode = 0 (standard), at least one question.
    if header[2] & 0xF8 != 0 || header[4..6] == [0, 0] {
        return None;
    }
    let question = &query[HEADER_LEN..question_end(query)?];
    let qtype = u16::from_be_bytes([question[question.len() - 4], question[question.len() - 3]]);
    let answers = u16::from(qtype == TYPE_A);

    let mut reply = Vec::with_capacity(HEADER_LEN + question.len() + 16);
    reply.extend(&header[..2]);
    // QR and RA set, RD echoed from the query.
    reply.extend([0x80 | (header[2] & 0x01), 0x80]);
    reply.extend([0, 1]);
    reply.extend(answers.to_be_bytes());
    reply.extend([0, 0, 0, 0]);
    reply.extend(question);
    if answers > 0 {
        reply.extend([0xC0, HEADER_LEN as u8]); // pointer to the question name
        reply.extend(TYPE_A.to_be_bytes());
        reply.extend(CLASS_IN.to_be_bytes());
        reply.extend(TTL_SECONDS.to_be_bytes());
        reply.extend(4u16.to_be_bytes());
        reply.extend(ip);
    }
    Some(reply)
}

/// End of the first question: its labels, the root label, type and class.
fn question_end(packet: &[u8]) -> Option<usize> {
    let mut i = HEADER_LEN;
    loop {
        let len = *packet.get(i)? as usize;
        if len == 0 {
            break;
        }
        i += 1 + len;
    }
    let end = i + 1 + 4;
    (end <= packet.len()).then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP: [u8; 4] = [192, 168, 71, 1];

    /// A standard query for `name` with recursion desired.
    fn query(name: &str, qtype: u16, additional: u16) -> Vec<u8> {
        let mut q = vec![0xAB, 0xCD, 0x01, 0x00, 0, 1, 0, 0, 0, 0];
        q.extend(additional.to_be_bytes());
        for label in name.split('.') {
            q.push(label.len() as u8);
            q.extend(label.as_bytes());
        }
        q.push(0);
        q.extend(qtype.to_be_bytes());
        q.extend(CLASS_IN.to_be_bytes());
        if additional > 0 {
            // EDNS OPT record, as sent by most resolvers.
            q.extend([0, 0, 41, 0x10, 0, 0, 0, 0, 0, 0, 0]);
        }
        q
    }

    fn question(name: &str, qtype: u16) -> Vec<u8> {
        query(name, qtype, 0)[HEADER_LEN..].to_vec()
    }

    #[test]
    fn a_query_is_answered_with_our_address() {
        let reply = answer(&query("connectivitycheck.gstatic.com", TYPE_A, 0), IP).unwrap();
        let mut expected = vec![0xAB, 0xCD, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        expected.extend(question("connectivitycheck.gstatic.com", TYPE_A));
        expected.extend([0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 192, 168, 71, 1]);
        assert_eq!(reply, expected);
    }

    #[test]
    fn additional_records_in_the_query_are_dropped() {
        let reply = answer(&query("apple.com", TYPE_A, 1), IP).unwrap();
        assert_eq!(&reply[6..12], &[0, 1, 0, 0, 0, 0]);
        assert!(reply.ends_with(&[192, 168, 71, 1]));
    }

    #[test]
    fn other_query_types_get_no_answer() {
        let reply = answer(&query("example.com", 28, 0), IP).unwrap();
        let mut expected = vec![0xAB, 0xCD, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0];
        expected.extend(question("example.com", 28));
        assert_eq!(reply, expected);
    }

    #[test]
    fn responses_are_not_answered() {
        let mut q = query("example.com", TYPE_A, 0);
        q[2] |= 0x80;
        assert_eq!(answer(&q, IP), None);
    }

    #[test]
    fn truncated_packets_are_not_answered() {
        let q = query("example.com", TYPE_A, 0);
        for len in [0, 5, HEADER_LEN, q.len() - 1] {
            assert_eq!(answer(&q[..len], IP), None, "len {len}");
        }
    }

    #[test]
    fn packets_without_questions_are_not_answered() {
        let mut q = query("example.com", TYPE_A, 0);
        q[5] = 0;
        assert_eq!(answer(&q, IP), None);
    }
}
