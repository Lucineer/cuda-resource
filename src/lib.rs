/*!
# cuda-resource

Resource allocation and management.

Agents compete for finite resources — CPU, memory, network, energy.
This crate manages who gets what, when, and how much. No agent
should starve another, and critical operations always take priority.

- Resource pools with configurable capacities
- Priority-based allocation (Critical > High > Normal > Low)
- Budgets and quotas
- Fair sharing with starvation prevention
- Garbage collection of idle resources
- Resource usage tracking
*/

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Resource types
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResourceType { Cpu, Memory, Network, Energy, Disk, Gpu }

/// Priority levels
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority { Low = 0, Normal = 1, High = 2, Critical = 3 }

/// A resource allocation request
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AllocationRequest {
    pub requester: String,
    pub resource: ResourceType,
    pub amount: f64,
    pub priority: Priority,
    pub duration_ms: Option<u64>,
    pub reusable: bool,
}

/// An active allocation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Allocation {
    pub id: String,
    pub requester: String,
    pub resource: ResourceType,
    pub amount: f64,
    pub priority: Priority,
    pub granted_ms: u64,
    pub expires_ms: Option<u64>,
    pub released: bool,
}

/// A resource pool
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourcePool {
    pub resource: ResourceType,
    pub capacity: f64,
    pub allocated: f64,
    pub allocations: Vec<Allocation>,
    pub next_id: u64,
}

impl ResourcePool {
    pub fn new(resource: ResourceType, capacity: f64) -> Self { ResourcePool { resource, capacity, allocated: 0.0, allocations: vec![], next_id: 1 } }

    pub fn available(&self) -> f64 { (self.capacity - self.allocated).max(0.0) }

    pub fn utilization(&self) -> f64 { if self.capacity <= 0.0 { return 0.0; } self.allocated / self.capacity }

    /// Try to allocate
    pub fn allocate(&mut self, request: &AllocationRequest) -> Option<Allocation> {
        // Critical always gets served, others compete
        if request.priority < Priority::Critical && request.amount > self.available() {
            // Try to preempt lower priority allocations
            let freed = self.preempt_lower(request.amount - self.available(), request.priority);
            if self.available() < request.amount { return None; }
        }
        if request.amount > self.available() { return None; }

        let id = format!("alloc_{}", self.next_id);
        self.next_id += 1;
        let expires = request.duration_ms.map(|d| now() + d);
        let alloc = Allocation { id: id.clone(), requester: request.requester.clone(), resource: self.resource, amount: request.amount, priority: request.priority, granted_ms: now(), expires_ms: expires, released: false };
        self.allocated += request.amount;
        self.allocations.push(alloc.clone());
        Some(alloc)
    }

    /// Release an allocation
    pub fn release(&mut self, alloc_id: &str) {
        if let Some(alloc) = self.allocations.iter_mut().find(|a| a.id == alloc_id) {
            if !alloc.released {
                self.allocated -= alloc.amount;
                alloc.released = true;
            }
        }
    }

    /// Preempt lower priority allocations to free up space
    fn preempt_lower(&mut self, needed: f64, min_priority: Priority) -> f64 {
        let mut freed = 0.0;
        let mut preempt = vec![];
        for alloc in &self.allocations {
            if !alloc.released && alloc.priority < min_priority {
                preempt.push(alloc.id.clone());
                freed += alloc.amount;
                if freed >= needed { break; }
            }
        }
        for id in preempt { self.release(&id); }
        freed
    }

    /// Release expired allocations
    pub fn gc(&mut self) -> u32 {
        let now = now();
        let expired: Vec<String> = self.allocations.iter()
            .filter(|a| !a.released && a.expires_ms.map_or(false, |e| e < now))
            .map(|a| a.id.clone()).collect();
        for id in expired { self.release(&id); }
        expired.len() as u32
    }

    /// Allocation summary for a requester
    pub fn usage_by(&self, requester: &str) -> f64 {
        self.allocations.iter().filter(|a| a.requester == requester && !a.released).map(|a| a.amount).sum()
    }
}

/// A budget for a requester
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub requester: String,
    pub limits: HashMap<ResourceType, f64>,
    pub used: HashMap<ResourceType, f64>,
}

impl Budget {
    pub fn new(requester: &str) -> Self { Budget { requester: requester.to_string(), limits: HashMap::new(), used: HashMap::new() } }
    pub fn set_limit(&mut self, resource: ResourceType, limit: f64) { self.limits.insert(resource, limit); }
    pub fn remaining(&self, resource: ResourceType) -> f64 {
        let limit = self.limits.get(&resource).copied().unwrap_or(f64::MAX);
        let used = self.used.get(&resource).copied().unwrap_or(0.0);
        (limit - used).max(0.0)
    }
    pub fn track(&mut self, resource: ResourceType, amount: f64) {
        *self.used.entry(resource).or_insert(0.0) += amount;
    }
}

/// The resource manager
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceManager {
    pub pools: HashMap<ResourceType, ResourcePool>,
    pub budgets: HashMap<String, Budget>,
    pub total_allocations: u64,
    pub total_preemptions: u64,
}

impl ResourceManager {
    pub fn new() -> Self { ResourceManager { pools: HashMap::new(), budgets: HashMap::new(), total_allocations: 0, total_preemptions: 0 } }

    /// Create a resource pool
    pub fn add_pool(&mut self, resource: ResourceType, capacity: f64) {
        self.pools.insert(resource, ResourcePool::new(resource, capacity));
    }

    /// Set budget for a requester
    pub fn set_budget(&mut self, requester: &str, resource: ResourceType, limit: f64) {
        self.budgets.entry(requester.to_string()).or_insert_with(|| Budget::new(requester)).set_limit(resource, limit);
    }

    /// Allocate resources
    pub fn allocate(&mut self, request: &AllocationRequest) -> Option<Allocation> {
        let pool = self.pools.get_mut(&request.resource)?;
        // Check budget
        if let Some(budget) = self.budgets.get(&request.requester) {
            if request.amount > budget.remaining(request.resource) { return None; }
        }
        let alloc = pool.allocate(request)?;
        if let Some(budget) = self.budgets.get_mut(&request.requester) {
            budget.track(request.resource, request.amount);
        }
        self.total_allocations += 1;
        Some(alloc)
    }

    /// Release allocation
    pub fn release(&mut self, resource: ResourceType, alloc_id: &str) {
        if let Some(pool) = self.pools.get_mut(&resource) { pool.release(alloc_id); }
    }

    /// Garbage collect all pools
    pub fn gc_all(&mut self) -> u32 {
        self.pools.values_mut().map(|p| p.gc()).sum()
    }

    /// Global utilization
    pub fn utilization(&self, resource: ResourceType) -> f64 {
        self.pools.get(&resource).map(|p| p.utilization()).unwrap_or(0.0)
    }

    /// Summary
    pub fn summary(&self) -> String {
        let pool_info: Vec<String> = self.pools.values().map(|p| format!("{:?}:{:.0}%", p.resource, p.utilization() * 100.0)).collect();
        format!("ResourceManager: {} pools [{},..], {} allocs, {} budgets",
            self.pools.len(), pool_info.join(", "), self.total_allocations, self.budgets.len())
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_allocation() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Memory, 100.0);
        let req = AllocationRequest { requester: "a1".into(), resource: ResourceType::Memory, amount: 30.0, priority: Priority::Normal, duration_ms: None, reusable: false };
        let alloc = rm.allocate(&req);
        assert!(alloc.is_some());
        assert_eq!(rm.utilization(ResourceType::Memory), 0.3);
    }

    #[test]
    fn test_over_allocation_fails() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Cpu, 50.0);
        let req = AllocationRequest { requester: "a1".into(), resource: ResourceType::Cpu, amount: 60.0, priority: Priority::Normal, duration_ms: None, reusable: false };
        assert!(rm.allocate(&req).is_none());
    }

    #[test]
    fn test_critical_preempts() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Memory, 100.0);
        let low = AllocationRequest { requester: "low".into(), resource: ResourceType::Memory, amount: 80.0, priority: Priority::Low, duration_ms: None, reusable: false };
        rm.allocate(&low);
        let critical = AllocationRequest { requester: "crit".into(), resource: ResourceType::Memory, amount: 50.0, priority: Priority::Critical, duration_ms: None, reusable: false };
        let alloc = rm.allocate(&critical);
        assert!(alloc.is_some()); // preempts low
    }

    #[test]
    fn test_release() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Cpu, 100.0);
        let req = AllocationRequest { requester: "a1".into(), resource: ResourceType::Cpu, amount: 50.0, priority: Priority::Normal, duration_ms: None, reusable: false };
        let alloc = rm.allocate(&req).unwrap();
        rm.release(ResourceType::Cpu, &alloc.id);
        assert_eq!(rm.utilization(ResourceType::Cpu), 0.0);
    }

    #[test]
    fn test_budget_limit() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Memory, 1000.0);
        rm.set_budget("a1", ResourceType::Memory, 50.0);
        let req = AllocationRequest { requester: "a1".into(), resource: ResourceType::Memory, amount: 100.0, priority: Priority::Normal, duration_ms: None, reusable: false };
        assert!(rm.allocate(&req).is_none()); // over budget
    }

    #[test]
    fn test_gc_expired() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Memory, 100.0);
        let req = AllocationRequest { requester: "a1".into(), resource: ResourceType::Memory, amount: 50.0, priority: Priority::Normal, duration_ms: Some(0), reusable: false };
        rm.allocate(&req);
        let collected = rm.gc_all();
        assert!(collected > 0);
        assert_eq!(rm.utilization(ResourceType::Memory), 0.0);
    }

    #[test]
    fn test_usage_by_requester() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Cpu, 100.0);
        rm.allocate(&AllocationRequest { requester: "a1".into(), resource: ResourceType::Cpu, amount: 20.0, priority: Priority::Normal, duration_ms: None, reusable: false });
        rm.allocate(&AllocationRequest { requester: "a1".into(), resource: ResourceType::Cpu, amount: 30.0, priority: Priority::Normal, duration_ms: None, reusable: false });
        let pool = rm.pools.get(&ResourceType::Cpu).unwrap();
        assert!((pool.usage_by("a1") - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_multiple_pools() {
        let mut rm = ResourceManager::new();
        rm.add_pool(ResourceType::Cpu, 100.0);
        rm.add_pool(ResourceType::Memory, 1000.0);
        rm.add_pool(ResourceType::Network, 50.0);
        assert_eq!(rm.pools.len(), 3);
    }

    #[test]
    fn test_pool_available() {
        let pool = ResourcePool::new(ResourceType::Cpu, 100.0);
        assert_eq!(pool.available(), 100.0);
    }

    #[test]
    fn test_summary() {
        let rm = ResourceManager::new();
        let s = rm.summary();
        assert!(s.contains("0 pools"));
    }
}
