import base, { prepareJourneySnapshot, usesProject } from "./playwright.base";
export { fixtureWebServer, journeyWebServer } from "./playwright.base";

if (usesProject("journeys")) prepareJourneySnapshot();
export default base;
