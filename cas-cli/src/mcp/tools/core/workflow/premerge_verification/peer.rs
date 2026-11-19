//! Legacy peer approval messages are readable but carry no decision authority.
use crate::cloud::peer_mailbox::ClaimedPeerMessage;

pub(crate) const TAG: &str = "CAS_PEER_PREMERGE_V1 ";

pub(crate) fn legacy_peer_approval(message: &ClaimedPeerMessage) -> bool {
    crate::cloud::peer_mailbox::split_message_intent(&message.body).1.starts_with(TAG)
}
