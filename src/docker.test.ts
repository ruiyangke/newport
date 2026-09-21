import { expect, it } from "vitest";
import { groupContainers, type Container } from "./docker";
const c = (
  name: string,
  project = "",
  service = "",
  oneoff = "False",
): Container => ({
  id: name,
  name,
  image: "test:1",
  state: "running",
  status: "Up",
  ports: "",
  composeProject: project,
  composeService: service,
  composeOneoff: oneoff,
});
it("groups by Compose labels rather than guessing from names", () => {
  const result = groupContainers([
    c("misleading-project-api-1"),
    c("custom-name", "shop", "api"),
    c("replica2", "shop", "api"),
    c("job", "shop", "task", "True"),
    c("db", "other", "db"),
  ]);
  expect(result.projects.map((p) => p.name)).toEqual(["other", "shop"]);
  expect(result.projects[1].services).toEqual(["api"]);
  expect(result.projects[1].containers).toHaveLength(3);
  expect(result.standalone[0].name).toBe("misleading-project-api-1");
});
