import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const readText = async (file) => (await fs.readFile(file, 'utf8')).replace(/\r\n?/g, '\n');
const read = (file) => readText(path.join(root, file));

async function walk(folder) {
  const entries = await fs.readdir(folder, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const full = path.join(folder, entry.name);
    if (entry.isDirectory()) files.push(...await walk(full));
    else if (entry.isFile()) files.push(full);
  }
  return files.sort();
}

const language = JSON.parse(await read('conformance/language.json'));
const sampleDir = path.join(root, 'samples');
const samplePaths = (await fs.readdir(sampleDir))
  .filter((name) => name.endsWith('.azo')).sort();
const samples = await Promise.all(samplePaths.map(async (name) => ({
  name: path.parse(name).name,
  source: await readText(path.join(sampleDir, name)),
})));

const fixtureDir = path.join(root, 'playground', 'fixtures');
const files = Object.create(null);
for (const file of await walk(fixtureDir)) {
  const name = path.relative(fixtureDir, file).split(path.sep).join('/');
  files[name] = await readText(file);
}

await fs.writeFile(path.join(root, 'playground', 'data.json'),
  JSON.stringify({ language, samples, files }), 'utf8');
console.log('Packaged playground/data.json');
