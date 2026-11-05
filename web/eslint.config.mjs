import nextVitals from "eslint-config-next/core-web-vitals";
import nextTs from "eslint-config-next/typescript";

const eslintConfig = [
  ...nextVitals,
  ...nextTs,
  {
    rules: {
      // Untrusted text rule: never render raw HTML.
      "react/no-danger": "error",
    },
  },
  { ignores: [".next/**", "out/**", "next-env.d.ts", "src/lib/api/schema.d.ts", "playwright-report/**", "test-results/**"] },
];

export default eslintConfig;
