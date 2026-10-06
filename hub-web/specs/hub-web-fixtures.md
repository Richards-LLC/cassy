# Cassy Commander Interactive Fixture Test Plan

## Application Overview

Base URL: the fixture server playwright.config.ts starts on this checkout's own port (e2e/checkout-ports.mjs; override with HUB_E2E_PORT). Every scenario starts from a fresh browser page, selects its fixture with /?fixture=<name>, uses Seed e2e/seed.spec.ts, and can run in any order. The plan covers the requested fixture states with happy paths, invalid input, and failure-state affordances. Commander shell fixtures render production UI with placeholder callbacks for backend actions; assertions stop at observable fixture behavior. A scenario fails whenever any listed expectation is unmet.

## Test Scenarios

### 1. Conversation composer

**Seed:** `e2e/seed.spec.ts`

#### 1.1. Composer preserves and edits the seeded draft

**File:** `e2e/generated/conversation-composer-draft.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=conversation-composer.
    - expect: The patient-pelican-9 conversation is open and the Your message textbox contains “Cut 3.26.0 once the gate is green, then post the release notes.”
    - expect: The send button is named Send to patient-pelican-9 and the attachment button is disabled.
  2. Replace the textbox contents with “Please verify the gate first.”
    - expect: The textbox value is exactly the replacement text and remains editable.
    - expect: The conversation log still contains the earlier “Rebased and pushed; nothing waiting.” reply.
  3. Clear the textbox.
    - expect: The textbox is empty without changing the earlier conversation; fail if draft edits alter the log or enable the unsupported attachment.

### 2. Conversation question

**Seed:** `e2e/seed.spec.ts`

#### 2.1. Fix in-train quick reply answers the pending question

**File:** `e2e/generated/conversation-ask-fix-option.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=conversation-ask.
    - expect: A Waiting on you region contains the gate-run question and buttons Fix in-train and Ship with allowlist.
    - expect: The thread contains the related blocker and attention.rs:212 evidence.
  2. Click Fix in-train once.
    - expect: The question in the log shows “You replied: Fix in-train”.
    - expect: A new operator turn says Fix in-train with a Sending… status.
    - expect: The pinned Waiting on you region and context attention list disappear; fail if the other option is submitted or the question remains unanswered.

### 3. Conversation question

**Seed:** `e2e/seed.spec.ts`

#### 3.1. Ship with allowlist quick reply records the alternate answer

**File:** `e2e/generated/conversation-ask-allowlist-option.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=conversation-ask and locate the pinned gate-run question.
    - expect: Both response options are present and no reply has been recorded yet.
  2. Click Ship with allowlist once.
    - expect: The question in the log shows “You replied: Ship with allowlist”.
    - expect: A new operator turn says Ship with allowlist with a Sending… status.
    - expect: The pinned question is removed; fail if Fix in-train appears as the chosen answer.

### 4. Conversation question

**Seed:** `e2e/seed.spec.ts`

#### 4.1. Collapsed gate updates expand and collapse without losing the pending ask

**File:** `e2e/generated/conversation-ask-status-history.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=conversation-ask.
    - expect: The gate progress appears as one condensed update with a Show full update button.
    - expect: The pending question remains in the Waiting on you region.
  2. Click Show full update.
    - expect: The history shows both “Gate started · 0 of 14 targets” and “gate 11 of 14 targets green”.
    - expect: The toggle becomes Show less and the pending question remains available.
  3. Click Show less.
    - expect: The gate progress is condensed again and Show full update returns; fail if the pending ask or blocker disappears.

### 5. Pairing entry

**Seed:** `e2e/seed.spec.ts`

#### 5.1. Pairing entry exposes exact scopes and validates optional email

**File:** `e2e/generated/pairing-step-1-email-validation.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=pairing-step-1.
    - expect: The Pair a machine modal is open.
    - expect: It leads with what this browser will be able to do, then the ten-minute code explanation, an optional "Email me the code too" field, and Create pairing code; the Cassy Cloud origin and the six exact scopes sit under a collapsed Technical details.
  2. Fill Email me the code too (optional) with “not-an-email” and click Create pairing code.
    - expect: The browser reports the email input invalid and retains the dialog on step 1; fail if the malformed value passes native email validation.
  3. Replace the field with “<operator@example.com>”.
    - expect: The field value is valid and the Create pairing code control remains available; this fixture does not supply a relay response to assert a later step.

### 6. Pairing entry

**Seed:** `e2e/seed.spec.ts`

#### 6.1. Escape dismisses the pairing modal

**File:** `e2e/generated/pairing-step-1-escape.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=pairing-step-1.
    - expect: The Pair a machine dialog is open over the conversation list, which is inert.
  2. Press Escape.
    - expect: The dialog closes and the conversation list and its Pair a machine control become accessible; fail if the modal remains open or traps focus.

### 7. Failed connection

**Seed:** `e2e/seed.spec.ts`

#### 7.1. Failed connection explains the third attempt and offers recovery controls

**File:** `e2e/generated/connection-failed-retry-actions.spec.ts`

**Steps:**

  1. From a fresh page, navigate to /?fixture=connection-failed-retry.
    - expect: The page says “Connection failed — retry available.”
    - expect: Connection attempts lists two earlier attempts, failed Attempt 3, and “The machine did not answer its hub address.”
  2. Locate and click Retry, then locate and click Diagnose.
    - expect: Both recovery controls are visible and enabled and can be activated without a page error.
    - expect: The failure explanation remains readable; the fixture stubs recovery callbacks, so no successful reconnection or diagnostic navigation is expected.
