<script>
  import { languages } from './index.js';
  import { preference, t } from './svelte.js';
</script>

<label class="language-select">
  <span>{$t('Language')}</span>
  <select bind:value={$preference} data-language-select>
    <option value="auto">{$t('Follow system')}</option>
    {#each languages as language}
      <option value={language.code} lang={language.code}>{language.name}</option>
    {/each}
  </select>
</label>
