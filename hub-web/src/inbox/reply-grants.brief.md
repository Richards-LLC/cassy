# Ask to reply — cas-874b

## Single idea

A browser reading a retained conversation can ask its account owner for permission to reply, keep its messages and start replying after approval.

## Hero form

The existing conversation composer carries one Ask to reply action and a plain pending, denied or expired status. The owner's inbox has a headed request list naming the browser and conversation beside Allow and Deny.

Owner decisions use the existing dialog action row: its space-2 gap separates Allow from Deny, and Allow uses the existing primary button fill. The row wraps at phone width while each native button keeps its label and keyboard action together.

## Emotional register

Explicit permission with no forced sign-out or lost history, using Commander's existing Glass sheet, headings and button tokens from hub-web/DESIGN.md and src/tokens.css.

## Distinctive move

Approval changes the same Cloud device grant; the conversation and its stored messages stay open. Pending never looks like permission, and absent Cloud support offers no dead control.

## Deliberately omitted

No local hub pairing privilege, automatic owner approval, broad command scope, new sign-in or delivery guarantee. The feature is dormant until Cloud advertises reply-grant-upgrade-v1. Proposal: petra-stella-cloud#174; production activation awaits an agreed/shipped contract.

## Components and state

ReplyGrantRequests owns typed receipts, request/decision retry IDs and server request state. OperatorInboxController owns the verified persisted identity, refreshes grants/me before polling requests/feed, and updates the PoP generation only after storage succeeds. InboxView renders requester status and the owner's approval list. Protocol double owns authorization, expiry, CAS and idempotency; it never treats a button click as permission.

States: unsupported (existing guidance), ready-to-ask, pending, approved (current scope enables composer), denied/expired (honest status and explicit new request), unavailable (retry without granting). Native buttons support Enter; status uses a live region. No new motion, fonts, images or token values.
