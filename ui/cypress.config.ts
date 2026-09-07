import { defineConfig } from "cypress"

export default defineConfig({
  allowCypressEnv: false,
  e2e: {
    baseUrl: "http://127.0.0.1:4173",
    fixturesFolder: "../mockserver",
    supportFile: false,
  },
})
