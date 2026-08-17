import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const readmePath = resolve(repositoryRoot, 'README.md');

function readReadme(): string {
  return readFileSync(readmePath, 'utf8');
}

describe('project README', () => {
  it('contains the required Chinese sections and verified commands', () => {
    const readme = readReadme();

    for (const heading of [
      '# 小桌宠',
      '## 当前可用',
      '## 可插件化桌宠',
      '## 快速开始',
      '## 隐私',
      '## 项目结构',
      '## OpenSpec 开发方式',
      '## 路线图',
      '## 资源授权与贡献'
    ]) {
      expect(readme).toContain(heading);
    }

    for (const command of [
      'bun install',
      'bun run tauri:dev',
      'bun run check',
      'bun test',
      'bun run build',
      'bun run tauri:build'
    ]) {
      expect(readme).toContain(command);
    }
  });

  it('keeps README media and repository links relative and present', () => {
    const readme = readReadme();
    const media = [
      ...readme.matchAll(
        /<img\s+[^>]*\bsrc="(docs\/assets\/readme\/[^"\s]+)"[^>]*\balt="([^"]+)"[^>]*>/g
      )
    ];

    expect(media).toHaveLength(3);
    for (const [, path, alt] of media) {
      expect(alt.trim()).not.toHaveLength(0);
      expect(path).not.toMatch(/^(?:[a-z]+:|\/)/i);
      expect(existsSync(resolve(repositoryRoot, path))).toBe(true);
    }

    const links = [...readme.matchAll(/(?<!!)\[[^\]]+\]\(([^)#?\s]+)\)/g)];
    expect(links.length).toBeGreaterThan(0);
    for (const [, path] of links) {
      expect(path).not.toMatch(/^(?:[a-z]+:|\/)/i);
      expect(existsSync(resolve(repositoryRoot, path))).toBe(true);
    }
  });

  it('marks unfinished plugin work as development or planning', () => {
    const readme = readReadme();

    expect(readme).toContain('小桌宠 Core Host');
    expect(readme).toContain('状态：开发中');
    expect(readme).toContain('.petpack 与本地目录（开发中）');
    expect(readme).toContain('Bun / TypeScript SDK（规划中）');
    expect(readme).toContain('add-plugin-system-foundation');
  });
});
