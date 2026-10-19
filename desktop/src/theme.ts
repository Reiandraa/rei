export type Theme = 'light' | 'dark';
export const ACCENTS = [
  {value:'green',name:'Sage',color:'#739a78'},
  {value:'ocean',name:'Ocean',color:'#6e9ebb'},
  {value:'lavender',name:'Lavender',color:'#9a89b7'},
  {value:'rose',name:'Rose',color:'#c08b9e'},
  {value:'sunrise',name:'Sunrise',color:'#cf9564'},
  {value:'peach',name:'Peach',color:'#d5a08e'},
  {value:'teal',name:'Teal',color:'#6d9e98'},
  {value:'sand',name:'Sand',color:'#b7a473'},
] as const;
export type Accent = typeof ACCENTS[number]['value'];
export function resolveTheme(theme: unknown): Theme {
  if(theme==='dark' || theme==='black')return 'dark'; // Read the retired preference without restoring its palette.
  if(theme==='light')return 'light';
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}
export function applyTheme(theme: unknown): void {document.documentElement.setAttribute('data-theme',resolveTheme(theme));}
export function resolveAccent(accent:unknown): Accent {return ACCENTS.find(a=>a.value===accent)?.value ?? 'green';}
export function applyAccent(accent: unknown): void {document.documentElement.setAttribute('data-accent',resolveAccent(accent));}
export function applyTextSize(size: unknown): void {document.documentElement.setAttribute('data-text-size',['small','standard','large'].includes(String(size))?String(size):'standard');}
export function applyMotion(motion: unknown): void {document.documentElement.setAttribute('data-motion',motion==='reduced'?'reduced':'system');}
