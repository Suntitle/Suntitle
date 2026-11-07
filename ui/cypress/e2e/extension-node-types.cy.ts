import type { Extension, NodeResource } from "../../src/stores/models"

/** Provide an extension whose display name differs from its resource identity. */
function extension(settings: Extension["spec"]["adapter"]["settings"] = {}): Extension {
  return {
    apiVersion: "control.suntitle.io/v1alpha1", kind: "SuntitleExtension",
    metadata: { name: "hello", generation: 1 },
    spec: { name: "Hello every 10 seconds", sourceType: "local", source: "/opt/suntitle/extensions/hello",
      adapter: { type: "translator", execution: { runtime: "python", command: "./hello.py" }, settings } },
  }
}

/** Model persistence so navigation and subsequent settings edits read the created custom node. */
function openHost(item: Extension) {
  const nodes: NodeResource[] = []
  cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" })
  cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" }).as("hosts")
  cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" }).as("types")
  cy.intercept("GET", "**/api/v1/extensions", { body: { items: [item], resourceVersion: "1" } }).as("extensions")
  cy.intercept("GET", "**/api/v1/nodes", (request) => request.reply({ items: nodes, resourceVersion: "1" })).as("nodes")
  cy.intercept("POST", "**/api/v1/hosts/*/nodes", (request) => {
    const resource = { metadata: { name: "hello-node", uid: "hello-node-uid" }, spec: request.body.spec }
    nodes.push(resource)
    request.reply({ statusCode: 201, body: resource })
  }).as("install")
  cy.intercept("PUT", "**/api/v1/nodes/hello-node", (request) => {
    nodes[0] = { ...nodes[0], spec: request.body.spec }
    request.reply(nodes[0])
  }).as("save")
  cy.visit("/hosts", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", "mock-token") } })
  cy.wait("@hosts")
  cy.contains("button", "studio-host").click()
  cy.wait(["@types", "@extensions", "@nodes"])
  cy.get('button[aria-label="Node type"]').should("contain.text", "Transcriber").click()
  cy.get('[data-slot="select-content"]').filter(":visible").should("have.length", 1).within(() => {
    cy.contains('[data-slot="select-item"]', "Transcriber").should("be.visible")
    cy.contains('[data-slot="select-item"]', "Hello every 10 seconds · Extension").click()
  })
}

describe("Extension node types", () => {
  it("installs Hello from the existing type selector without input or built-in translator settings", () => {
    openHost(extension())
    cy.get('button[aria-label="Node type"]').should("contain.text", "Hello every 10 seconds")
    cy.get("#node-name").should("have.value", "hello")
    cy.get("#node-setting-sourceLanguage, #node-setting-targetLanguage").should("not.exist")
    cy.contains("button", "Install node").click()
    cy.wait("@install").then(({ request }) => {
      expect(request.body).to.deep.equal({ spec: {
        name: "hello", enabled: true, hostRef: { name: "studio-host-resource" }, inputRef: null,
        extensionRef: { name: "hello" }, settings: { custom: {} },
      } })
    })
    cy.get('[role="status"]').should("contain.text", "installation was requested")
    cy.contains("button", "Cancel").click()
    cy.get('a[href="/nodes"]').first().click()
    cy.get('[data-testid="topology-node"]').should("have.length", 1).and("contain.text", "hello")
    cy.get('button[aria-label^="Open settings for hello,"]').click()
    cy.get('[data-slot="dialog-title"]').should("have.text", "Update hello")
    cy.contains("button", "Save changes").click()
    cy.wait("@save").its("request.body.spec").should("include", { enabled: true })
    cy.get("@save").its("request.body.spec.settings").should("deep.equal", { custom: {} })
    cy.get("@save").its("request.body.spec.extensionRef").should("deep.equal", { name: "hello" })
  })

  it("uses manifest defaults and preserves arbitrary JSON custom settings", () => {
    cy.viewport(1440, 1000)
    openHost(extension({ message: { default: "Hello" }, enabled: { default: false }, nullable: { default: null }, nested: { default: { values: [1, true] } }, token: { required: true } }))
    cy.get("#node-setting-message").should("have.value", "Hello").clear().type("Hi")
    cy.get("#node-setting-nullable").should("have.value", "null")
    cy.get("#node-setting-nested").should("have.value", '{"values":[1,true]}')
    cy.get("#node-setting-token").type('"secret"')
    cy.get("#node-setting-nested").clear().type("invalid")
    cy.contains("button", "Install node").click()
    cy.get('[role="alert"]').should("contain.text", "nested must be valid JSON")
    cy.get("#node-setting-nested").clear().type('{"values":[1,true]}', { parseSpecialCharSequences: false })
    cy.contains("button", "Install node").click()
    cy.wait("@install").its("request.body.spec.settings").should("deep.equal", {
      custom: { message: "Hi", enabled: false, nullable: null, nested: { values: [1, true] }, token: "secret" },
    })
  })

  it("keeps built-in types available when extension loading fails", () => {
    cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" })
    cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" })
    cy.intercept("GET", "**/api/v1/nodes", { body: { items: [] } })
    cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" })
    cy.intercept("GET", "**/api/v1/extensions", { statusCode: 503, body: { error: { message: "Registry unavailable" } } })
    cy.visit("/hosts", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", "mock-token") } })
    cy.contains("button", "studio-host").click()
    cy.get('[role="alert"]').should("contain.text", "Unable to load extension types")
    cy.get('button[aria-label="Node type"]').click()
    cy.contains('[data-slot="select-item"]', "Audio source").click()
    cy.get('button[aria-label="Node type"]').should("contain.text", "Audio source")
  })
})
