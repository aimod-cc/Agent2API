import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import dotenv from 'dotenv';
import Store from 'electron-store';
import { getUserDataPath } from './config-path.js';
import { CampusDeviceIdService } from './campus-device-id-service.js';
import { getEnvFileCandidates } from './utils/env-file-helpers.js';
import { decryptEnvContent } from './utils/env-file-crypto.js';
import { normalizeBaseUrl, normalizeString } from './utils/string-helpers.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const PROJECT_ROOT = path.resolve(__dirname, '..');

const DEFAULT_BASE_URL = 'https://ossptest.voicecloud.cn/loomy/integration';
const DEFAULT_CONFIG_FILE_PATH = path.resolve(PROJECT_ROOT, 'config', 'points.config.local.json');

function readJsonFile(filePath) {
  try {
    if (!fs.existsSync(filePath)) {
      return {};
    }

    const content = fs.readFileSync(filePath, 'utf-8');
    const parsed = JSON.parse(content);
    return parsed && typeof parsed === 'object' ? parsed : {};
  } catch (error) {
    console.warn('[PointsConfig] 读取积分配置文件失败:', filePath, error);
    return {};
  }
}

function readEnvFile(filePath) {
  try {
    if (!fs.existsSync(filePath)) {
      return {};
    }

    const content = decryptEnvContent(fs.readFileSync(filePath, 'utf-8'));
    return dotenv.parse(content);
  } catch (error) {
    console.warn('[PointsConfig] 读取环境配置文件失败:', filePath, error);
    return {};
  }
}

function getSearchRoots() {
  return [
    PROJECT_ROOT,
    process.cwd(),
    process.resourcesPath,
  ].filter(Boolean);
}

function resolveEnvFileConfig() {
  const envFileCandidates = getEnvFileCandidates({
    explicitEnvPath: normalizeString(process.env.LOOMY_POINTS_ENV_PATH || process.env.LOOMY_ENV_PATH),
    searchRoots: getSearchRoots(),
  });

  let resolvedPath = envFileCandidates[0] || '';
  let values = {
    baseUrl: '',
    token: '',
  };

  for (const envPath of envFileCandidates) {
    const envValues = readEnvFile(envPath);
    const nextValues = {
      baseUrl: normalizeString(
        envValues.LOOMY_POINTS_BASE_URL
        || envValues.VITE_LOOMY_POINTS_BASE_URL
      ),
      token: normalizeString(
        envValues.LOOMY_POINTS_TOKEN
        || envValues.VITE_POINTS_TOKEN
        || envValues.LOOMY_ASR_TOKEN
        || envValues.VITE_ASR_TOKEN
      ),
    };

    if (!nextValues.baseUrl && !nextValues.token) {
      continue;
    }

    resolvedPath = envPath;
    values = nextValues;
    break;
  }

  return {
    path: resolvedPath,
    values,
  };
}

function resolveFileConfig() {
  const envConfigPath = normalizeString(process.env.LOOMY_POINTS_CONFIG_PATH);
  const configPath = envConfigPath || DEFAULT_CONFIG_FILE_PATH;
  const fileConfig = readJsonFile(configPath);

  return {
    path: configPath,
    values: {
      baseUrl: normalizeString(fileConfig.baseUrl),
      token: normalizeString(fileConfig.token || fileConfig.asrToken),
    },
  };
}

export class PointsConfig {
  constructor() {
    this.store = new Store({
      cwd: getUserDataPath(),
      name: 'points-config',
      defaults: {
        baseUrl: DEFAULT_BASE_URL,
        token: '',
        campusDeviceId: '',
      },
    });
    this.campusDeviceIdService = new CampusDeviceIdService({
      store: this.store,
    });
  }

  getConfig() {
    const envFileConfig = resolveEnvFileConfig();
    const fileConfig = resolveFileConfig();
    const resolvedBaseUrl =
      normalizeBaseUrl(
        process.env.LOOMY_POINTS_BASE_URL
        || process.env.VITE_LOOMY_POINTS_BASE_URL
        || envFileConfig.values.baseUrl
      )
      || normalizeBaseUrl(fileConfig.values.baseUrl)
      || normalizeBaseUrl(this.store.get('baseUrl'))
      || DEFAULT_BASE_URL;

    return {
      envFilePath: envFileConfig.path,
      configFilePath: fileConfig.path,
      baseUrl: resolvedBaseUrl,
      token:
        normalizeString(
          process.env.LOOMY_POINTS_TOKEN
          || process.env.VITE_POINTS_TOKEN
          || process.env.LOOMY_ASR_TOKEN
          || process.env.VITE_ASR_TOKEN
          || envFileConfig.values.token
        )
        || normalizeString(fileConfig.values.token)
        || normalizeString(this.store.get('token')),
    };
  }

  saveConfig({ baseUrl, token }) {
    if (baseUrl) {
      this.store.set('baseUrl', normalizeBaseUrl(baseUrl));
    }
    if (typeof token === 'string') {
      this.store.set('token', normalizeString(token));
    }
  }

  getCampusDeviceId() {
    return this.campusDeviceIdService.getDeviceId();
  }
}

export const pointsConfig = new PointsConfig();
