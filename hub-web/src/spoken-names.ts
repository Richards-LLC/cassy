/**
 * Accessible names in plain words (cas-d8a5, journey F32): a supervisor is
 * "the <project> supervisor", never its generated codename as the speaker,
 * and spoken parts join with ", " only after text that does not already end
 * a sentence, with no empty parts and no stray " , ".
 */

/** "cas-src supervisor", or the codename when no project is known. */
export function spokenSupervisor(project: string | undefined, codename: string): string {
  return project ? `${project} supervisor` : codename;
}

/** The codename as a description, when the name does not already say it. */
export function supervisorDescription(project: string | undefined, codename: string, otherSession = false): string | undefined {
  if (!project) return undefined;
  return otherSession ? `earlier session ${codename}` : codename;
}

/**
 * Join spoken parts: empty parts drop out; ", " follows a part unless it
 * already ends with sentence punctuation, where a space does.
 */
export function joinSpoken(parts: ReadonlyArray<string | undefined | null | false>): string {
  let spoken = "";
  for (const part of parts) {
    const text = (part || "").trim();
    if (!text) continue;
    if (!spoken) { spoken = text; continue; }
    spoken += /[.!?…:;,]$/.test(spoken) ? ` ${text}` : `, ${text}`;
  }
  return spoken;
}
