#!/usr/bin/env node
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const filename = process.argv[2];
if (!filename) throw new Error('source filename is required');
const require = createRequire(resolve(process.cwd(), 'hub-web/package.json'));
const ts = require('typescript');
const source = await new Promise((resolveSource, reject) => {
  let value = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', chunk => { value += chunk; });
  process.stdin.on('end', () => resolveSource(value));
  process.stdin.on('error', reject);
});
const kind = /\.tsx$/i.test(filename) ? ts.ScriptKind.TSX :
  /\.jsx$/i.test(filename) ? ts.ScriptKind.JSX : ts.ScriptKind.TS;
const tree = ts.createSourceFile(filename, source, ts.ScriptTarget.Latest, true, kind);
if (tree.parseDiagnostics.length) {
  throw new Error(`cannot parse ${filename}: ${tree.parseDiagnostics.map(d =>
    ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('; ')}`);
}

function rootIdentifier(expression) {
  while (ts.isCallExpression(expression) || ts.isPropertyAccessExpression(expression)
      || ts.isElementAccessExpression(expression) || ts.isNonNullExpression(expression)
      || ts.isParenthesizedExpression(expression) || ts.isAsExpression(expression)
      || ts.isTypeAssertionExpression(expression) || ts.isSatisfiesExpression(expression)) {
    expression = expression.expression;
  }
  return ts.isIdentifier(expression) ? expression.text : '';
}
function testCallName(call) {
  return rootIdentifier(call.expression);
}
function eachRoot(expression) {
  return ts.isPropertyAccessExpression(expression) && expression.name.text === 'each'
    ? rootIdentifier(expression.expression) : '';
}
function isEachChain(call) {
  const expression = ts.isCallExpression(call.expression) ? call.expression.expression : call.expression;
  return eachRoot(expression) !== '';
}
function staticTitle(expression) {
  if (ts.isStringLiteralLike(expression)) return expression.text;
  return null;
}
const calls = [];
function unwrap(node) {
  while (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isTypeAssertionExpression(node)
      || ts.isNonNullExpression(node) || ts.isSatisfiesExpression(node)) node = node.expression;
  return node;
}
function staticValue(raw) {
  const node = unwrap(raw);
  if (ts.isStringLiteralLike(node)) return node.text;
  if (ts.isNumericLiteral(node)) return Number(node.text);
  if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
  if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
  if (node.kind === ts.SyntaxKind.NullKeyword) return null;
  if (ts.isIdentifier(node) && node.text === 'undefined') return undefined;
  if (ts.isArrayLiteralExpression(node)) {
    const values = node.elements.map(staticValue);
    return values.some(value => value === DYNAMIC) ? DYNAMIC : values;
  }
  if (ts.isObjectLiteralExpression(node)) {
    const values = {};
    for (const property of node.properties) {
      if (!ts.isPropertyAssignment(property) || !property.name || !
          (ts.isIdentifier(property.name) || ts.isStringLiteralLike(property.name))) return DYNAMIC;
      const value = staticValue(property.initializer);
      if (value === DYNAMIC) return DYNAMIC;
      values[property.name.text] = value;
    }
    return values;
  }
  return DYNAMIC;
}
const DYNAMIC = Symbol('dynamic');
function expandTitle(format, value, index) {
  const args = Array.isArray(value) ? value : [value];
  let cursor = 0;
  const title = format.replace(/%%|%#|%[sdifjoO]/g, token => {
    if (token === '%%') return '%';
    if (token === '%#') return String(index);
    const item = args[cursor++];
    if (item === undefined) return 'undefined';
    return token === '%j' ? JSON.stringify(item) : String(item);
  });
  if (value && typeof value === 'object' && !Array.isArray(value)) {
    return title.replace(/\$([\w.]+)/g, (_whole, key) => {
      const found = key.split('.').reduce((item, part) => item?.[part], value);
      return found === undefined ? '$' + key : String(found);
    });
  }
  return title;
}
function testRows(call) {
  const outer = call.expression;
  if (!ts.isCallExpression(outer) || !ts.isPropertyAccessExpression(outer.expression)
      || outer.expression.name.text !== 'each' || outer.arguments.length !== 1) return null;
  const table = staticValue(outer.arguments[0]);
  if (!Array.isArray(table)) return null;
  const title = call.arguments.length ? staticTitle(call.arguments[0]) : null;
  if (title === null) return null;
  return table.map((row, index) => ({
    functionTitle: expandTitle(title, row, index),
    fingerprint: call.getText(tree) + '\nrow=' + JSON.stringify(row),
  }));
}
function walk(node, suites = []) {
  if (ts.isTaggedTemplateExpression(node)) {
    const name = eachRoot(node.tag);
    if (['test', 'it', 'describe', 'suite'].includes(name)) {
      throw new Error(`unresolved dynamic ${name === 'describe' || name === 'suite' ? 'suite' : 'test'} collection in ${filename}`);
    }
  }
  if (ts.isCallExpression(node)) {
    const name = testCallName(node);
    const knownTestRoot = ['test', 'it', 'describe', 'suite'].includes(name);
    const eachChain = isEachChain(node);
    const eachFactory = ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === 'each';
    if (knownTestRoot && eachChain && (name === 'describe' || name === 'suite')) {
      throw new Error(`unresolved dynamic suite collection in ${filename}`);
    }
    if (knownTestRoot && !eachFactory) {
      const rows = ['test', 'it'].includes(name) ? testRows(node) : null;
      const title = node.arguments.length ? staticTitle(node.arguments[0]) : null;
      if (rows) {
        const scopes = suites.map(item => item.title);
        for (const row of rows) calls.push({ function: [...scopes, row.functionTitle].join('::'),
          line: tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1,
          fingerprint: row.fingerprint });
      } else if (title === null) {
        if (name === 'test' || name === 'it') {
          throw new Error(`unresolved dynamic test title or collection in ${filename}`);
        } else if (name === 'describe' || name === 'suite') {
          throw new Error(`unresolved dynamic suite title in ${filename}`);
        }
      } else if (['test', 'it'].includes(name) && eachChain) {
        throw new Error(`unresolved dynamic test collection in ${filename}`);
      } else if (name === 'describe' || name === 'suite') {
        suites = [...suites, { start: node.getStart(tree), end: node.end, title }];
      } else {
        const scopes = suites.filter(item => item.start < node.getStart(tree) && node.end < item.end)
          .map(item => item.title);
        const functionName = [...scopes, title].join('::');
        calls.push({ function: functionName,
          line: tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1,
          fingerprint: node.getText(tree) });
      }
    }
    const chain = node.expression;
    if (knownTestRoot && ts.isPropertyAccessExpression(chain)
        && ['only', 'skip', 'concurrent', 'serial', 'fixme'].includes(chain.name.text)) {
      throw new Error(`unresolved dynamic test modifier ${chain.name.text} in ${filename}`);
    }
  }
  ts.forEachChild(node, child => walk(child, suites));
}
walk(tree);
const seen = new Set();
for (const test of calls) {
  if (seen.has(test.function)) throw new Error(`ambiguous test title ${test.function} in ${filename}`);
  seen.add(test.function);
}
process.stdout.write(JSON.stringify(calls));
