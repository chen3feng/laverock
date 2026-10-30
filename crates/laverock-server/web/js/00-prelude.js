const $ = (id) => document.getElementById(id);
const PAGES = ['synthesize', 'captures', 'voices', 'history', 'models', 'settings'];
const HF_PRESETS = [
  { value: '', labelKey: 'preset.official.hf' },
  { value: 'https://hf-mirror.com', labelKey: 'preset.cn' },
];
const PIP_PRESETS = [
  { value: '', labelKey: 'preset.official.pip' },
  { value: 'https://pypi.tuna.tsinghua.edu.cn/simple', label: '清华 TUNA' },
  { value: 'https://mirrors.aliyun.com/pypi/simple/', label: '阿里云' },
];
