#!/usr/bin/env node
'use strict'

const { spawnSync } = require('child_process')

const BINS = {
  'darwin-arm64': '@nijaru/tk-darwin-arm64/bin/tk',
  'darwin-x64':   '@nijaru/tk-darwin-x64/bin/tk',
  'linux-arm64':  '@nijaru/tk-linux-arm64/bin/tk',
  'linux-x64':    '@nijaru/tk-linux-x64/bin/tk',
  'win32-x64':    '@nijaru/tk-win32-x64/bin/tk.exe',
}

const key = `${process.platform}-${process.arch}`
const bin = BINS[key]

if (!bin) {
  process.stderr.write(`tk: unsupported platform ${key}\n`)
  process.exit(1)
}

let resolved
try {
  resolved = require.resolve(bin)
} catch {
  process.stderr.write(`tk: binary not found — try reinstalling: npm install -g @nijaru/tk\n`)
  process.exit(1)
}

process.exit(spawnSync(resolved, process.argv.slice(2), { stdio: 'inherit' }).status ?? 1)
