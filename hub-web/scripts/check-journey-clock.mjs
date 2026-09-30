#!/usr/bin/env node
// Journey data must use the injected clock; monotonic durations use
// performance.now(). Check syntax trees so comments/strings aren't violations.
import ts from 'typescript';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function member(node) {
  if (ts.isPropertyAccessExpression(node)) return { object: node.expression, name: node.name.text };
  if (ts.isElementAccessExpression(node) && node.argumentExpression && ts.isStringLiteral(node.argumentExpression)) {
    return { object: node.expression, name: node.argumentExpression.text };
  }
}
function date(node) {
  if (ts.isIdentifier(node) && node.text === 'Date') return true;
  const access = member(node);
  return access?.name === 'Date' && ts.isIdentifier(access.object) && ['globalThis', 'window'].includes(access.object.text);
}

export function clockViolations(text, file = 'journey.ts') {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true);
  const errors = [];
  function visit(node) {
    const access = member(node);
    const ambient = access?.name === 'now' && date(access.object)
      || ts.isNewExpression(node) && date(node.expression) && !node.arguments?.length
      || ts.isCallExpression(node) && date(node.expression)
      || ts.isCallExpression(node) && member(node.expression)?.name === 'install'
        && member(member(node.expression)?.object)?.name === 'clock' && !node.arguments.length;
    if (ambient) {
      const { line, character } = source.getLineAndCharacterOfPosition(node.getStart(source));
      errors.push(`${file}:${line + 1}:${character + 1}: ambient Date; use clock.ts for timestamps or performance.now() for elapsed time`);
    }
    ts.forEachChild(node, visit);
  }
  visit(source);
  return errors;
}

function checkDirectory(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) return checkDirectory(path);
    return entry.name.endsWith('.ts') ? clockViolations(readFileSync(path, 'utf8'), path) : [];
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = checkDirectory(fileURLToPath(new URL('../e2e/journeys', import.meta.url)));
  if (errors.length) {
    console.error(errors.join('\n'));
    process.exitCode = 1;
  } else console.log('journey-clock: PASS (all journey timestamps use the injected clock)');
}
