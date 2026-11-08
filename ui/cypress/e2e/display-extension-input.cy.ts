describe("Display extension inputs", () => {
  it("offers text producers and excludes incompatible extensions during creation and editing", () => {
    const nodes = ["text", "audio", "image", "unknown"].map((type) => ({
      name: `hello-${type}`, displayName: `Hello ${type}`, enabled: true,
      hostRef: { name: "studio-host-resource" }, extensionRef: { name: `hello-${type}` },
      settings: { custom: {} },
      status: { resolvedAdapter: { outputType: type === "unknown" ? undefined : type } },
    }))
    cy.intercept("GET", "**/api/v1/auth/me", { fixture: "api/v1/auth/me.json" })
    cy.intercept("GET", "**/api/v1/hosts", { fixture: "api/v1/hosts.json" })
    cy.intercept("GET", "**/api/v1/node-types", { fixture: "api/v1/node-types.json" })
    cy.intercept("GET", "**/api/v1/extensions", { body: { items: [] } })
    cy.intercept("GET", "**/api/v1/nodes", (request) => request.reply({ items: nodes }))
    cy.intercept("POST", "**/api/v1/hosts/*/nodes", (request) => {
      const resource = { ...request.body, name: "new-display" }
      nodes.push(resource)
      request.reply({ statusCode: 201, body: resource })
    }).as("install")
    cy.intercept("PUT", "**/api/v1/nodes/new-display", (request) => request.reply({ ...request.body, name: "new-display" })).as("save")
    cy.visit("/hosts", { onBeforeLoad(window) { window.localStorage.setItem("suntitle.accessToken", "mock-token") } })
    cy.contains("button", "studio-host").click()
    cy.get('button[aria-label="Node type"]').click()
    cy.contains('[data-slot="select-item"]', "Display").click()
    cy.get('button[aria-label="Input"]').click()
    cy.get('[data-slot="select-content"]').filter(":visible").should("contain.text", "Hello text")
      .and("not.contain.text", "Hello audio").and("not.contain.text", "Hello image").and("not.contain.text", "Hello unknown")
    cy.contains('[data-slot="select-item"]', "Hello text").click()
    cy.contains("button", "Install node").click()
    cy.wait("@install").its("request.body.inputRef").should("deep.equal", { name: "hello-text" })
    cy.contains("button", "Cancel").click()
    cy.get('a[href="/nodes"]').first().click()
    cy.get('button[aria-label^="Open settings for live-display,"]').click()
    cy.get('button[aria-label="Input"]').should("contain.text", "Hello text").click()
    cy.get('[data-slot="select-content"]').filter(":visible").should("contain.text", "Hello text").and("not.contain.text", "Hello audio")
    cy.contains('[data-slot="select-item"]', "Hello text").click()
    cy.contains("button", "Save changes").click()
    cy.wait("@save").its("request.body.inputRef").should("deep.equal", { name: "hello-text" })
  })
})
