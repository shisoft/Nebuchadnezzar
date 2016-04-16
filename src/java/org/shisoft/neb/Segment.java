package org.shisoft.neb;

import org.shisoft.neb.exceptions.StoreFullException;
import org.shisoft.neb.io.Writer;

import java.util.concurrent.ConcurrentSkipListSet;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.locks.ReentrantReadWriteLock;

/**
 * Created by shisoft on 16-4-11.
 */
public class Segment {

    Trunk trunk;
    private long baseAddr;
    private AtomicLong currentLoc;
    private AtomicInteger deadObjectBytes;
    private ReentrantReadWriteLock lock;
    private ConcurrentSkipListSet<Long> frags;

    public Segment(long baseAddr, Trunk trunk) {
        this.baseAddr = baseAddr;
        this.trunk = trunk;
        this.currentLoc = new AtomicLong(baseAddr);
        this.deadObjectBytes = new AtomicInteger(0);
        this.lock = new ReentrantReadWriteLock();
        this.frags = new ConcurrentSkipListSet<>();
    }

    public ReentrantReadWriteLock getLock() {
        return lock;
    }

    public long getBaseAddr() {
        return baseAddr;
    }

    public long getCurrentLoc() {
        return currentLoc.get();
    }

    public boolean resetCurrentLoc (long expected, long update) {
        return currentLoc.compareAndSet(expected, update);
    }

    public int getDeadObjectBytes() {
        return deadObjectBytes.get();
    }

    public int incDeadObjectBytes (int len) {
        return deadObjectBytes.addAndGet(len);
    }

    public int decDeadObjectBytes (int len) {
        return deadObjectBytes.addAndGet(-1 * len);
    }

    public float aliveDataRatio () {
        return 1 - ((float) getDeadObjectBytes()) / ((float) this.currentLoc.get());
    }

    public int getAliveObjectBytes () {
        return (int) (currentLoc.get() - baseAddr - deadObjectBytes.get());
    }

    public ConcurrentSkipListSet<Long> getFrags() {
        return frags;
    }

    public long tryAcquireSpace (long len) {
        try {
            lock.readLock().lock();
            AtomicBoolean updated = new AtomicBoolean(false);
            long r = this.currentLoc.getAndUpdate(originalLoc -> {
                long expectedLoc = originalLoc + len;
                long expectedPos = expectedLoc - baseAddr;
                if (expectedPos >= Trunk.getSegSize()) {
                    updated.set(false);
                    return originalLoc;
                } else {
                    updated.set(true);
                    return expectedLoc;
                }
            });
            if (updated.get()) {
                return r;
            } else {
                return -1;
            }
        } finally {
            lock.readLock().unlock();
        }
    }

    public void fillZero () {
        for (long i = baseAddr; i < Trunk.segSize; i ++){
            Writer.writeByte((byte) 0, i);
        }
    }

    @Override
    public boolean equals(Object o) {
        if (this == o) return true;
        if (o == null || getClass() != o.getClass()) return false;

        Segment segment = (Segment) o;

        return baseAddr == segment.baseAddr;

    }

    @Override
    public int hashCode() {
        return (int) (baseAddr ^ (baseAddr >>> 32));
    }
}
