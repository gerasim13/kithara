pub(crate) type TestPools = kithara_bufpool::testing::TestPools;

pub(crate) fn pools() -> kithara_bufpool::PoolRegion<TestPools> {
    kithara_bufpool::testing::pools()
}
