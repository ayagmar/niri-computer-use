--!strict
-- Groups the agents with a server on this desktop into the panel's list: the
-- driver first, then one row for each agent, project and build, so two Claude
-- Code sessions in one repository share a row that counts them.

local M = {}

-- One row of the list.
export type Group = {
  key: string,
  name: string,
  project: string?,
  -- The servers' process ids, as `watch` lists them.
  pids: { number },
  replaced: boolean,
  driving: boolean,
}

local function group(agent: any, driving: boolean): Group
  return {
    key = `agent-{agent.pid}`,
    name = agent.name,
    project = agent.project,
    pids = { agent.pid },
    replaced = agent.replaced,
    driving = driving,
  }
end

-- The row an agent other than the driver joins, if one is already listed.
local function sharing(groups: { Group }, agent: any): Group?
  for _, g in groups do
    if not g.driving and g.name == agent.name and g.project == agent.project and g.replaced == agent.replaced then
      return g
    end
  end
  return nil
end

-- `agents` as the desk keeps them; `driver` the pid of the server holding the
-- desktop, if any.
function M.groups(agents: { any }, driver: number?): { Group }
  local groups: { Group } = {}
  for _, agent in agents do
    if agent.pid == driver then
      table.insert(groups, group(agent, true))
    end
  end
  for _, agent in agents do
    if agent.pid ~= driver then
      local found = sharing(groups, agent)
      if found then
        table.insert(found.pids, agent.pid)
      else
        table.insert(groups, group(agent, false))
      end
    end
  end
  return groups
end

return M
